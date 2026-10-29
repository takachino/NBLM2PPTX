# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Compare pinned n2p revisions using isolated model caches and per-child rusage."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import tarfile
import time
import tomllib
from typing import TypedDict, cast
import xml.etree.ElementTree as ET
import zipfile


@dataclass
class Measurement:
    elapsed_seconds: float
    user_seconds: float
    system_seconds: float
    max_rss_bytes: int
    exit_code: int
    pid: int


class Trial(TypedDict):
    condition: str
    variant: str
    repetition: int
    measurement: dict[str, float | int]
    valid: bool


class Artifact(TypedDict):
    path: str
    sha256: str
    bytes: int


class Variant(TypedDict):
    commit: str
    source_archive: Artifact
    cargo_lock: Artifact
    binary: Artifact
    reference: Artifact


class Manifest(TypedDict):
    version: int
    variants: dict[str, Variant]
    source: Artifact
    model: Artifact
    fonts: list[Artifact]
    arguments: list[str]
    repetitions: int
    device: str
    preparation: dict[str, object]


def rss_bytes(value: int, system: str) -> int:
    if value < 0:
        raise ValueError("Negative RSS")
    if system == "Darwin":
        return value
    if system == "Linux":
        return value * 1024
    raise ValueError(f"Unsupported RSS unit on {system}; use macOS or Linux/WSL2")


def measure_process(args: list[str], env: dict[str, str], log_dir: Path) -> Measurement:
    if not hasattr(os, "wait4"):
        raise ValueError("wait4 is required (macOS or Linux/WSL2)")
    log_dir.mkdir(parents=True, exist_ok=False)
    with (log_dir / "stdout.log").open("wb") as stdout, (log_dir / "stderr.log").open("wb") as stderr:
        start = time.perf_counter_ns()
        process = subprocess.Popen(args, env=env, stdout=stdout, stderr=stderr)
        try:
            _, status, usage = os.wait4(process.pid, 0)
        except BaseException:
            process.terminate()
            process.wait()
            raise
        end = time.perf_counter_ns()
        process.returncode = os.waitstatus_to_exitcode(status)
    return Measurement((end-start)/1e9, usage.ru_utime, usage.ru_stime,
                       rss_bytes(usage.ru_maxrss, platform.system()), process.returncode, process.pid)


def summarize(trials: list[Trial], repetitions: int) -> dict[str, object]:
    if repetitions < 3:
        raise ValueError("At least three repetitions per variant and cache condition are required")
    expected = set(schedule(repetitions))
    seen: set[tuple[str, str, int]] = set()
    for trial in trials:
        key = (trial["condition"], trial["variant"], trial["repetition"])
        if key not in expected or key in seen:
            raise ValueError(f"Unexpected or duplicate trial: {key}")
        seen.add(key)
        measured = trial["measurement"]
        if not trial["valid"] or measured["exit_code"] != 0:
            raise ValueError(f"Failed or invalid trial: {key}")
        for field in ["elapsed_seconds", "max_rss_bytes"]:
            if not math.isfinite(measured[field]) or measured[field] <= 0:
                raise ValueError(f"Invalid measurement: {key}, {field}")
    if seen != expected:
        raise ValueError("Missing trials; a complete comparison cannot be reported")
    result: dict[str, object] = {}
    for condition in ["empty", "ready"]:
        variants: dict[str, dict[str, dict[str, object]]] = {}
        for variant in ["before", "after"]:
            samples = sorted((t for t in trials if t["condition"] == condition and t["variant"] == variant), key=lambda t: t["repetition"])
            variants[variant] = {}
            for field in ["elapsed_seconds", "max_rss_bytes", "user_seconds", "system_seconds"]:
                values = [t["measurement"][field] for t in samples]
                variants[variant][field] = {"median":statistics.median(values), "min":min(values), "max":max(values), "values":values}
        changes = {field:(float(variants["after"][field]["median"])/float(variants["before"][field]["median"])-1)*100 for field in ["elapsed_seconds", "max_rss_bytes"]}
        result[condition] = {**variants, "change_percent":changes}
    return result


def schedule(repetitions: int) -> list[tuple[str, str, int]]:
    result: list[tuple[str, str, int]] = []
    for repetition in range(1, repetitions+1):
        conditions = ["empty", "ready"] if repetition % 2 else ["ready", "empty"]
        for condition in conditions:
            variants = ["before", "after"] if (repetition + (condition=="ready")) % 2 else ["after", "before"]
            result.extend((condition,variant,repetition) for variant in variants)
    return result


def validate_pptx(actual: Path, expected: Path) -> dict[str, object]:
    def signature(path: Path) -> tuple[dict[str, str], int, int]:
        with zipfile.ZipFile(path) as archive:
            names = archive.namelist()
            if len(names) != len(set(names)):
                raise ValueError(f"Duplicate ZIP parts: {path}")
            parts = {name:hashlib.sha256(archive.read(name)).hexdigest() for name in names}
            presentation = ET.fromstring(archive.read("ppt/presentation.xml"))
            pages = sum(el.tag.endswith("}sldId") for el in presentation.iter())
            text_boxes = 0
            for name in names:
                if name.startswith("ppt/slides/slide") and name.endswith(".xml"):
                    slide = ET.fromstring(archive.read(name))
                    text_boxes += sum(el.tag.endswith("}sp") and any(c.tag.endswith("}t") for c in el.iter()) for el in slide.iter())
        if pages < 1 or text_boxes < 1:
            raise ValueError("Missing slides or editable text")
        return parts, pages, text_boxes
    try:
        actual_parts, pages, text_boxes = signature(actual)
        expected_parts, expected_pages, expected_boxes = signature(expected)
    except (OSError, ValueError, zipfile.BadZipFile, ET.ParseError, KeyError) as error:
        return {"valid":False,"error":str(error)}
    differing = sorted(name for name in actual_parts.keys() | expected_parts.keys() if actual_parts.get(name) != expected_parts.get(name))
    return {"valid":not differing and pages==expected_pages and text_boxes==expected_boxes,
            "pages":pages,"text_boxes":text_boxes,"matched_parts":len(actual_parts)-sum(name in actual_parts for name in differing),
            "differing_parts":differing,"sha256":digest(actual),"bytes":actual.stat().st_size,
            "zip_sha256_matches":digest(actual)==digest(expected)}


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream,"sha256").hexdigest()


def artifact(path: Path) -> Artifact:
    path = path.resolve()
    return {"path":str(path),"sha256":digest(path),"bytes":path.stat().st_size}


def verify(item: Artifact) -> Path:
    path = Path(item["path"])
    if digest(path) != item["sha256"] or path.stat().st_size != item["bytes"]:
        raise ValueError(f"Pinned artifact changed: {path}")
    return path


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value,ensure_ascii=False,indent=2,allow_nan=False)+"\n",encoding="utf-8")


def probe(args: list[str]) -> dict[str, object]:
    try:
        result = subprocess.run(args,capture_output=True,text=True,timeout=30,check=False)
        return {"command":args,"exit_code":result.returncode,"stdout":result.stdout.strip(),"stderr":result.stderr.strip()}
    except (OSError,subprocess.TimeoutExpired) as error:
        return {"command":args,"unavailable":str(error)}


def environment() -> dict[str, object]:
    values: dict[str, object] = {"utc":datetime.now(timezone.utc).isoformat(),"system":platform.platform(),"machine":platform.machine(),
        "python":platform.python_version(),"load_average":os.getloadavg(),
        "processes":probe(["ps","-A","-o","pid=,pcpu=,pmem=,comm="])}
    if platform.system() == "Darwin":
        values["power"] = probe(["pmset","-g","batt"])
        values["thermal"] = probe(["pmset","-g","therm"])
    return values


def prepare(args: argparse.Namespace) -> None:
    out: Path = args.out.resolve()
    out.mkdir(parents=True,exist_ok=False)
    repo: Path = args.repo.resolve()
    build_target: Path = (args.build_target or out / "build-target").resolve()
    variants: dict[str, Variant] = {}
    build_info: dict[str, object] = {"rustc":probe(["rustc","-Vv"]),"cargo":probe(["cargo","-V"]),"profile":"release","jobs":2,
        "target_root":str(build_target),"target_layout":"separate directory per revision to prevent cross-revision Cargo artifact reuse","flags":["--release","--locked","--offline","-j","2","-p","n2p-cli"],
        "environment":{k:v for k,v in os.environ.items() if k.startswith(("CARGO_","RUST"))},"builds":{}}
    locks: dict[str, set[tuple[str, str, str]]] = {}
    for name, revision, reference in [("before",args.before,args.before_reference),("after",args.after,args.after_reference)]:
        commit = subprocess.check_output(["git","rev-parse",f"{revision}^{{commit}}"],cwd=repo,text=True).strip()
        archive_path = out / f"{name}-source.tar"
        with archive_path.open("wb") as stream:
            subprocess.run(["git","archive","--format=tar",commit,"macos-app"],cwd=repo,stdout=stream,check=True)
        source = out / "sources" / name
        source.mkdir(parents=True)
        with tarfile.open(archive_path) as archive:
            archive.extractall(source,filter="data")
        workspace = source / "macos-app"
        lock = workspace / "Cargo.lock"
        lock_data = tomllib.loads(lock.read_text())
        locks[name] = {(p["name"],p["version"],p.get("source","workspace")) for p in lock_data["package"] if "source" in p}
        env = dict(os.environ)
        variant_target = build_target / f"performance-{name}"
        env["CARGO_TARGET_DIR"] = str(variant_target)
        print(f"Building {name}: {commit}",flush=True)
        started = time.perf_counter()
        with (out / f"build-{name}.log").open("wb") as stream:
            subprocess.run(["cargo","build","--manifest-path",str(workspace/"Cargo.toml"),"--release","--locked","--offline","-j","2","-p","n2p-cli"],env=env,stdout=stream,stderr=stream,check=True)
        cast(dict[str, object],build_info["builds"])[name] = {"seconds":time.perf_counter()-started,"log":f"build-{name}.log"}
        binary = out / "binaries" / name / "n2p"
        binary.parent.mkdir(parents=True)
        shutil.copy2(variant_target/"release/n2p",binary)
        variants[name] = {"commit":commit,"source_archive":artifact(archive_path),"cargo_lock":artifact(lock),"binary":artifact(binary),"reference":artifact(reference)}
    if locks["before"] != locks["after"]:
        raise ValueError("External dependency package versions differ; inspect the lockfiles before comparing")
    build_info["identical_external_dependency_packages"] = len(locks["before"])
    manifest: Manifest = {"version":1,"variants":variants,"source":artifact(args.input),"model":artifact(args.model),
        "fonts":[artifact(p) for p in args.font_file],"arguments":["--dpi","150","--layout","vision","--lang","ja-JP,en-US","--font","auto","--high-res-background","--erase","lama","--text-mode","erase","--method","background_smooth","--pages","1-14"],
        "repetitions":args.repetitions,"device":args.device,"preparation":build_info}
    write_json(out / "manifest.json",manifest)
    print(out / "manifest.json",flush=True)


def inventory(root: Path) -> list[dict[str, object]]:
    return [{"path":str(path.relative_to(root)),"bytes":path.stat().st_size,"sha256":digest(path)}
            for path in sorted(root.rglob("*")) if path.is_file()]


def model_cache(root: Path, model: Path) -> Path:
    root.mkdir(parents=True,exist_ok=False)
    path = root / "lama_fp32.onnx"
    shutil.copyfile(model,path)
    return path


def check_cache(root: Path, ready: bool) -> None:
    converted = root / "lama_fp32.coreml-v1.onnx"
    compiled = root / "coreml/v1"
    if ready:
        if not converted.is_file() or not compiled.is_dir() or not any(p.is_file() for p in compiled.rglob("*")):
            raise ValueError(f"Prepared CoreML cache is missing: {root}")
    elif converted.exists() or compiled.exists():
        raise ValueError(f"Expected a fresh model conversion cache: {root}")


def measure(manifest_path: Path, results: Path) -> None:
    manifest = cast(Manifest,json.loads(manifest_path.read_text(encoding="utf-8")))
    if manifest["version"] != 1 or set(manifest["variants"]) != {"before","after"} or manifest["repetitions"] < 3:
        raise ValueError("Invalid benchmark manifest")
    if manifest["device"] != "gpu" or platform.system() != "Darwin":
        raise ValueError("This manifest measures CoreML GPU on macOS; define a separate comparison for other devices/OSes")
    source, original_model = verify(manifest["source"]),verify(manifest["model"])
    pinned = [manifest["source"],manifest["model"],*manifest["fonts"]]
    for variant in manifest["variants"].values():
        pinned.extend([variant["binary"],variant["reference"],variant["cargo_lock"],variant["source_archive"]])
    for item in pinned:
        verify(item)
    results.mkdir(parents=True,exist_ok=False)
    env_base = dict(os.environ)
    env_base["N2P_LAMA_DEVICE"] = manifest["device"]
    system_info: dict[str, object] = {"initial":environment(),"hardware":probe(["sysctl","-n","hw.model","hw.memsize","hw.ncpu","machdep.cpu.brand_string"]),
        "app_nap":"No App Nap setting changed; fresh command-line children; no per-process App Nap measurement",
        "power_management":"No system settings changed; power and thermal status saved per trial",
        "os_file_cache":"Not cleared; hash verification and model copying may warm file caches",
        "other_caches":"CoreML internal shared caches and Vision system caches are not cleared",
        "device":"N2P_LAMA_DEVICE=gpu; CoreML MLProgram CPUAndGPU; fallback is rejected; per-operator hardware utilization is not profiled",
        "memory_scope":"Peak RSS of the CLI child (not total system memory or external CoreML/Vision service memory)",
        "measurement_scope":"CLI spawn through exit; includes PDF/OCR/model loading/conversion/PPTX writing, excludes build/copy/hash/cache warmup/output validation",
        "manifest_sha256":digest(manifest_path),"script_sha256":digest(Path(__file__)),"manifest":manifest}
    write_json(results / "environment.json",system_info)
    trials: list[Trial] = []
    records: list[dict[str, object]] = []

    def execute(condition: str, variant_name: str, repetition: int, cache: Path, name: str) -> Trial:
        variant = manifest["variants"][variant_name]
        root = results / name
        root.mkdir()
        env = dict(env_base)
        env["N2P_LAMA_MODEL"] = str(cache / "lama_fp32.onnx")
        before_state = inventory(cache)
        write_json(root / "environment-before.json",environment())
        write_json(root / "cache-before.json",before_state)
        output = root / "output.pptx"
        cmd = [variant["binary"]["path"],"convert",str(source),"-o",str(output),*manifest["arguments"]]
        measured = measure_process(cmd,env,root / "process")
        # 計測後のハッシュや検証による CPU とメモリ使用量は、child の rusage に入らない。
        write_json(root / "environment-after.json",environment())
        after_state = inventory(cache)
        write_json(root / "cache-after.json",after_state)
        quality = validate_pptx(output,Path(variant["reference"]["path"]))
        stderr = (root / "process/stderr.log").read_text(encoding="utf-8",errors="replace")
        fallback = any(token.lower() in stderr.lower() for token in ["Warning:","LaMa 不可用","LaMa 無法","改用","退回"])
        try:
            check_cache(cache,True)
            backend_ready = True
        except ValueError:
            backend_ready = False
        valid = measured.exit_code==0 and quality["valid"] is True and not fallback and backend_ready
        trial: Trial = {"condition":condition,"variant":variant_name,"repetition":repetition,"measurement":asdict(measured),"valid":valid}
        record: dict[str, object] = {**trial,"command":cmd,"environment":{"N2P_LAMA_DEVICE":env["N2P_LAMA_DEVICE"],"N2P_LAMA_MODEL":env["N2P_LAMA_MODEL"]},
            "quality":quality,"fallback_detected":fallback,"coreml_cache_ready":backend_ready,"directory":str(root)}
        write_json(root / "result.json",record)
        records.append(record)
        write_json(results / "records.json",records)
        print(f"{name}: {measured.elapsed_seconds:.3f} s, {measured.max_rss_bytes/1024**3:.3f} GiB, valid={valid}",flush=True)
        if not valid:
            raise ValueError(f"Invalid conversion; inspect {root / 'result.json'} and process/stderr.log")
        return trial

    for variant in ["before","after"]:
        cache = results / "caches" / f"ready-{variant}"
        model_cache(cache,original_model)
        print(f"Preparing excluded warmup: {variant}",flush=True)
        execute("warmup",variant,0,cache,f"warmup-{variant}")
    order = schedule(manifest["repetitions"])
    write_json(results / "schedule.json",order)
    for index,(condition,variant,repetition) in enumerate(order,1):
        cache = results / "caches" / (f"ready-{variant}" if condition=="ready" else f"empty-{repetition:02d}-{variant}")
        if condition == "empty":
            model_cache(cache,original_model)
        check_cache(cache,condition=="ready")
        print(f"[{index}/{len(order)}] {condition} {variant} repetition {repetition}",flush=True)
        trials.append(execute(condition,variant,repetition,cache,f"{index:02d}-{condition}-{variant}"))
    for item in pinned:
        verify(item)
    summary = summarize(trials,manifest["repetitions"])
    write_json(results / "summary.json",{"complete":True,"units":{"elapsed_seconds":"seconds","max_rss_bytes":"bytes"},"conditions":summary,"trials":trials})
    print(results / "summary.json",flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command",required=True)
    preparation = commands.add_parser("prepare",help="Export and build pinned revisions; never modifies the source commits")
    preparation.add_argument("--repo",type=Path,required=True)
    preparation.add_argument("--out",type=Path,required=True)
    preparation.add_argument("--build-target",type=Path)
    preparation.add_argument("--before",required=True)
    preparation.add_argument("--after",required=True)
    preparation.add_argument("--before-reference",type=Path,required=True)
    preparation.add_argument("--after-reference",type=Path,required=True)
    preparation.add_argument("--input",type=Path,required=True)
    preparation.add_argument("--model",type=Path,required=True)
    preparation.add_argument("--font-file",type=Path,action="append",required=True)
    preparation.add_argument("--repetitions",type=int,default=3)
    preparation.add_argument("--device",choices=["gpu"],default="gpu")
    measurement = commands.add_parser("measure",help="Run isolated conversions and write raw measurements and a summary")
    measurement.add_argument("--manifest",type=Path,required=True)
    measurement.add_argument("--results",type=Path,required=True)
    args = parser.parse_args()
    if args.command=="prepare":
        prepare(args)
    else:
        measure(args.manifest.resolve(),args.results.resolve())


if __name__=="__main__":
    main()
