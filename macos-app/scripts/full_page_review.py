# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow==13.0.0"]
# ///
"""Render pinned source/before/after artifacts and build a local quality review.

Usage: uv run scripts/full_page_review.py --manifest review.json --out review --xtask target/debug/xtask
"""

from __future__ import annotations

import argparse
import hashlib
import html
import json
import os
from pathlib import Path
import platform
import posixpath
import shutil
import subprocess
import tempfile
from typing import NotRequired, TypedDict, cast
import xml.etree.ElementTree as ET
import zipfile
from io import BytesIO

from PIL import Image, ImageDraw, ImageFont


class Artifact(TypedDict):
    path: str
    sha256: str
    commit: str | None


class Region(TypedDict):
    name: str
    page: int
    rect: list[int]


class Manifest(TypedDict):
    version: int
    pages: int
    canvas_px: list[int]
    artifacts: dict[str, Artifact]
    evidence: list[Artifact]
    regions: list[Region]
    evaluation_config: str
    notes: list[str]
    fontconfig: NotRequired[Artifact]


def read_json(path: Path) -> object:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verify(base: Path, artifact: Artifact) -> dict[str, object]:
    path = (base / artifact["path"]).resolve()
    actual = digest(path)
    if actual != artifact["sha256"]:
        raise ValueError(f"SHA-256 mismatch: {path}")
    return {"path": str(path), "sha256": actual, "bytes": path.stat().st_size, "commit": artifact["commit"]}


def render_environment(base: Path, artifact: Artifact | None) -> tuple[dict[str, str], dict[str, object] | None]:
    env = dict(os.environ)
    if artifact is None:
        inherited = env.get("FONTCONFIG_FILE")
        if inherited:
            path = Path(inherited).resolve()
            return env, {"path":str(path),"sha256":digest(path),"source":"inherited_environment"}
        return env, None
    record = verify(base, artifact)
    env["FONTCONFIG_FILE"] = str(record["path"])
    env["FONTCONFIG_PATH"] = str(Path(str(record["path"])).parent)
    return env, record


def command(args: list[str], env: dict[str, str] | None = None) -> str:
    result = subprocess.run(args, check=True, capture_output=True, text=True, timeout=180, env=env)
    return result.stdout.strip() + result.stderr.strip()


def soffice_path() -> str:
    candidates = [os.environ.get("SOFFICE_PATH"), shutil.which("soffice"), "/Applications/LibreOffice.app/Contents/MacOS/soffice"]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return str(Path(candidate).resolve())
    raise FileNotFoundError("LibreOffice is required; set SOFFICE_PATH to an existing soffice executable")


def render_pdf(pdf: Path, work: Path, dpi: int, pages: int, env: dict[str, str]) -> list[Path]:
    work.mkdir(parents=True)
    command(["pdftoppm", "-r", str(dpi), "-png", str(pdf), str(work / "page")], env)
    images = sorted(work.glob("page-*.png"), key=lambda p: int(p.stem.rsplit("-", 1)[1]))
    if len(images) != pages:
        raise ValueError(f"Expected {pages} pages, got {len(images)}: {pdf}")
    if [int(p.stem.rsplit("-", 1)[1]) for p in images] != list(range(1, pages + 1)):
        raise ValueError(f"Missing or duplicate page numbers: {pdf}")
    return images


def render_pptx(binary: str, pptx: Path, work: Path, pages: int, env: dict[str, str]) -> tuple[Path, list[Path]]:
    work.mkdir(parents=True)
    # 毎回新しいディレクトリへコピーし、過去の PDF の残存を成功と誤認しない。
    safe = work / "input.pptx"
    shutil.copyfile(pptx, safe)
    command([binary, "--headless", "--norestore", f"-env:UserInstallation={(work / 'profile').as_uri()}", "--convert-to", "pdf", "--outdir", str(work), str(safe)], env)
    pdf = work / "input.pdf"
    if not pdf.is_file():
        raise FileNotFoundError(f"LibreOffice did not produce a PDF for {pptx}")
    return pdf, render_pdf(pdf, work / "png", 144, pages, env)


def backgrounds(pptx: Path, out: Path, role: str, canvas: tuple[int, ...], pages: int) -> None:
    ns = {"p":"http://schemas.openxmlformats.org/presentationml/2006/main", "a":"http://schemas.openxmlformats.org/drawingml/2006/main", "r":"http://schemas.openxmlformats.org/officeDocument/2006/relationships"}
    rid = f'{{{ns["r"]}}}id'

    def resolve(host: str, target: str) -> str:
        return target.lstrip("/") if target.startswith("/") else posixpath.normpath(posixpath.join(posixpath.dirname(host),target))

    def rels(archive: zipfile.ZipFile, host: str) -> dict[str, str]:
        parent, filename = posixpath.split(host)
        root = ET.fromstring(archive.read(f"{parent}/_rels/{filename}.rels"))
        return {el.attrib["Id"]:resolve(host,el.attrib["Target"]) for el in root if el.get("TargetMode") != "External"}

    out.mkdir(exist_ok=True)
    with zipfile.ZipFile(pptx) as archive:
        presentation = ET.fromstring(archive.read("ppt/presentation.xml"))
        mapping = rels(archive,"ppt/presentation.xml")
        slides = presentation.findall("p:sldIdLst/p:sldId", ns)
        if len(slides) != pages:
            raise ValueError("PPTX slide count does not match the manifest")
        for page, slide_id in enumerate(slides, 1):
            part = mapping[slide_id.attrib[rid]]
            slide = ET.fromstring(archive.read(part))
            blip = slide.find("p:cSld/p:bg/p:bgPr/a:blipFill/a:blip", ns)
            if blip is None:
                raise ValueError(f"Missing image background: {pptx}, page {page}")
            image_part = rels(archive,part)[blip.attrib[f'{{{ns["r"]}}}embed']]
            with Image.open(BytesIO(archive.read(image_part))) as image:
                image.convert("RGB").resize(canvas,Image.Resampling.LANCZOS).save(out / f"{role}-{page:02d}.png")


def resolve_evaluation(config: Path, out: Path) -> dict[str, object]:
    value = cast(dict[str, object], read_json(config))
    fields = {"image_diff":["actual","expected","mask"],"luminance":["image"],"mask_overlap":["preserved","erasure"],"text":["pptx"],"runs":["pptx"],"bounds":["pptx","measurements","diagnostics"],"json_equal":["actual"]}
    for check in cast(list[dict[str, object]],value["checks"]):
        metric = cast(dict[str, object],check["metric"])
        for key in fields.get(str(metric["kind"]),[]):
            path = metric.get(key)
            if isinstance(path,str):
                metric[key] = str(out / path.removeprefix("@render/")) if path.startswith("@render/") else str((config.parent / path).resolve())
    return value


def triptych(images: list[Image.Image], title: str) -> Image.Image:
    width, height = images[0].size
    if any(image.size != (width, height) for image in images) or len(images) != 3:
        raise ValueError("A comparison requires three equally sized images")
    gap, top, bottom = 16, 84, 16
    output = Image.new("RGB", (width * 3 + gap * 4, height + top + bottom), "#edf1f6")
    draw = ImageDraw.Draw(output)
    font = ImageFont.load_default(size=22)
    draw.text((gap, 10), title, font=font, fill="#14273d")
    for column, (image, label) in enumerate(zip(images, ["Original PDF", "Before | LibreOffice", "After | LibreOffice"], strict=True)):
        x = gap + column * (width + gap)
        draw.text((x, 48), label, font=font, fill="#334b65")
        output.paste(image, (x, top))
    return output


def make_html(out: Path, manifest: Manifest, result: dict[str, object], report: dict[str, object]) -> None:
    pages = manifest["pages"]
    notes = "".join(f"<li>{html.escape(note)}</li>" for note in manifest["notes"])
    rows = []
    for check in cast(list[dict[str, object]], report["checks"]):
        status = str(check["status"])
        detail = check.get("error") or json.dumps(check["values"], ensure_ascii=False)
        rows.append(f'<tr><td>{html.escape(str(check["category"]))}</td><td>{html.escape(str(check["name"]))}</td><td class="{status}">{status}</td><td>{html.escape(str(detail))}</td></tr>')
    page_sections = "".join(f'<section id="page-{page:02d}"><h2>{page:02d} / {pages:02d}</h2><a href="pages/page-{page:02d}.png"><img loading="lazy" src="pages/page-{page:02d}.png" alt="{page} ページの元 PDF、修正前、修正後"></a></section>' for page in range(1, pages + 1))
    region_sections = []
    for index, region in enumerate(manifest["regions"], 1):
        stem = f"region-{index:02d}"
        region_sections.append(f'<section><h3>{html.escape(region["name"])} (p.{region["page"]})</h3><p>原寸 / <a href="regions/{stem}-200.png">200% PNG</a></p><div class="scroll"><img class="native" loading="lazy" src="regions/{stem}-100.png" alt="優先領域の原寸比較"></div><details><summary>200% で表示</summary><div class="scroll"><img class="native" loading="lazy" src="regions/{stem}-200.png" alt="優先領域の 200% 比較"></div></details></section>')
    summary = html.escape(json.dumps(report["summary"], ensure_ascii=False, indent=2))
    provenance = html.escape(json.dumps(result, ensure_ascii=False, indent=2))
    title = "全ページの完成表示比較と領域別評価"
    document = f'''<!doctype html><html lang="ja"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title>
<style>body{{font-family:system-ui,sans-serif;margin:0;background:#f3f6fa;color:#162c42;line-height:1.7}}main{{max-width:1520px;margin:auto;padding:28px}}h1{{font-size:28px}}h2{{border-bottom:2px solid #b9c9db;padding-bottom:8px}}section,header{{background:white;border:1px solid #d5dfe9;border-radius:12px;padding:22px;margin-bottom:24px}}img{{display:block;width:100%;height:auto}}img.native{{width:auto;max-width:none}}.scroll{{overflow:auto}}nav{{display:flex;gap:12px;flex-wrap:wrap}}a{{color:#1761a2}}table{{border-collapse:collapse;width:100%;font-size:13px}}td,th{{border:1px solid #ccd8e5;padding:8px;text-align:left;vertical-align:top}}td:last-child{{max-width:650px;overflow-wrap:anywhere}}.passed{{color:#126542}}.failed{{color:#a12b22}}.unmeasured{{color:#8b5c0e}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}@media print{{section{{break-inside:avoid}}}}</style>
<main><header><h1>{title}</h1><p>左から元 PDF / 修正前 / 修正後。PPTX は LibreOffice による文字と背景を含む完成表示。</p><ul>{notes}</ul><nav>{" ".join(f'<a href="#page-{page:02d}">{page:02d}</a>' for page in range(1,pages+1))}<a href="#regions">領域拡大</a><a href="#evaluation">評価</a><a href="#provenance">取得条件</a></nav></header>
<section><h2>全 {pages} ページの一覧</h2><a href="overview.png"><img src="overview.png" alt="全ページの三者比較一覧"></a></section>{page_sections}
<section id="regions"><h2>優先領域と対照領域</h2>{"".join(region_sections)}</section>
<section id="evaluation"><h2>自動評価</h2><p>passed / failed / unmeasured を区別する。詳細な指標としきい値は <a href="evaluation.json">JSON</a> に記録。</p><pre>{summary}</pre><details><summary>全項目の結果</summary><table><thead><tr><th>分類</th><th>評価名</th><th>状態</th><th>値または未測定の理由</th></tr></thead><tbody>{"".join(rows)}</tbody></table></details></section>
<section id="provenance"><h2>元ファイルと取得条件</h2><pre>{provenance}</pre></section></main></html>'''
    (out / "index.html").write_text(document, encoding="utf-8")


def build(manifest_path: Path, out: Path, xtask: Path) -> int:
    manifest = cast(Manifest, read_json(manifest_path))
    if manifest["version"] != 1 or manifest["pages"] <= 0:
        raise ValueError("Invalid manifest version or page count")
    base = manifest_path.parent
    canvas = tuple(manifest["canvas_px"])
    if len(canvas) != 2 or any(v <= 0 for v in canvas):
        raise ValueError("Invalid comparison canvas")
    for region in manifest["regions"]:
        l, t, r, b = region["rect"]
        if not (1 <= region["page"] <= manifest["pages"] and 0 <= l < r <= canvas[0] and 0 <= t < b <= canvas[1]):
            raise ValueError(f"Invalid region: {region}")
    if set(manifest["artifacts"]) != {"source", "before", "after", "approved_baseline"}:
        raise ValueError("Source, before, after, and approved_baseline artifacts are required")
    artifacts = {name: verify(base, artifact) for name, artifact in manifest["artifacts"].items()}
    evidence = [verify(base, artifact) for artifact in manifest["evidence"]]
    env, fontconfig = render_environment(base, manifest.get("fontconfig"))
    evaluation_config = (base / manifest["evaluation_config"]).resolve()
    config_sha256 = digest(evaluation_config)
    binary = soffice_path()
    version = command([binary, "--version"])
    poppler_version = command(["pdftoppm", "-v"])
    out.mkdir(parents=True, exist_ok=True)
    (out / "pages").mkdir(exist_ok=True)
    (out / "regions").mkdir(exist_ok=True)
    (out / "rendered").mkdir(exist_ok=True)
    normalized: dict[str, list[Path]] = {}
    render_records: dict[str, list[dict[str, object]]] = {}
    with tempfile.TemporaryDirectory(prefix="n2p-full-review-") as temp:
        work = Path(temp)
        for name in ["source", "before", "after"]:
            print(f"Rendering {name}: {manifest['pages']} pages", flush=True)
            path = Path(str(artifacts[name]["path"]))
            if name == "source":
                rendered = render_pdf(path, work / name, 72, manifest["pages"], env)
            else:
                pdf, rendered = render_pptx(binary, path, work / name, manifest["pages"], env)
                shutil.copyfile(pdf, out / "rendered" / f"{name}.pdf")
            target = out / "rendered" / name
            target.mkdir(exist_ok=True)
            normalized[name], render_records[name] = [], []
            for page, image_path in enumerate(rendered, 1):
                with Image.open(image_path) as source:
                    native = source.size
                    if name == "source" and native != canvas:
                        raise ValueError(f"Source page {page} is {native}, expected {canvas} at 72 DPI")
                    image = source.convert("RGB").resize(canvas, Image.Resampling.LANCZOS)
                destination = target / f"page-{page:02d}.png"
                image.save(destination)
                normalized[name].append(destination)
                render_records[name].append({"page":page,"native_px":native,"comparison_px":canvas,"scale_xy":[canvas[0]/native[0],canvas[1]/native[1]],"sha256":digest(destination)})
    for name in ["before", "after"]:
        backgrounds(Path(str(artifacts[name]["path"])),out / "backgrounds",name,canvas,manifest["pages"])
    for page in range(1, manifest["pages"] + 1):
        images = [Image.open(normalized[name][page - 1]) for name in ["source", "before", "after"]]
        try:
            triptych(images, f"Page {page:02d} / {manifest['pages']:02d} | Completed slide rendering").save(out / "pages" / f"page-{page:02d}.png")
        finally:
            for image in images:
                image.close()
    for index, region in enumerate(manifest["regions"], 1):
        images = []
        for name in ["source", "before", "after"]:
            with Image.open(normalized[name][region["page"] - 1]) as image:
                images.append(image.crop(tuple(region["rect"])))
        for zoom in [1, 2]:
            enlarged = [image.resize((image.width * zoom, image.height * zoom), Image.Resampling.NEAREST) for image in images]
            triptych(enlarged, f"Page {region['page']:02d} | Region {index:02d} | {zoom * 100}%").save(out / "regions" / f"region-{index:02d}-{zoom*100}.png")
    thumbs = []
    for page in range(1, manifest["pages"] + 1):
        with Image.open(out / "pages" / f"page-{page:02d}.png") as image:
            height = round(image.height * 1440 / image.width)
            thumbs.append(image.resize((1440, height), Image.Resampling.LANCZOS))
    overview = Image.new("RGB", (1440, sum(image.height for image in thumbs)), "white")
    y = 0
    for image in thumbs:
        overview.paste(image, (0, y))
        y += image.height
    overview.save(out / "overview.png")
    resolved_config = out / "resolved-evaluation-config.json"
    write_json(resolved_config,resolve_evaluation(evaluation_config,out))
    result: dict[str, object] = {"manifest_sha256":digest(manifest_path),"evaluation_config_sha256":config_sha256,"resolved_evaluation_config_sha256":digest(resolved_config),"xtask_sha256":digest(xtask),"artifacts":artifacts,"evidence":evidence,"fontconfig":fontconfig,"os":platform.platform(),"libreoffice":version,"poppler":poppler_version,"pillow":Image.__version__,"rule":{"source_dpi":72,"pptx_pdf_dpi":144,"canvas_px":canvas,"resize":"independent x/y scaling, LANCZOS; no crop or padding","region_zoom":"nearest neighbour 200%"},"rendered_pages":render_records}
    with tempfile.TemporaryDirectory(prefix="evaluate-", dir=out) as evaluation_work:
        fresh_report = Path(evaluation_work) / "evaluation.json"
        evaluated = subprocess.run([str(xtask), "quality-eval", "--config", str(resolved_config), "--report", str(fresh_report)], check=False)
        if not fresh_report.is_file():
            raise RuntimeError("Quality evaluation did not produce a report")
        fresh_report.replace(out / "evaluation.json")
    report = cast(dict[str, object], read_json(out / "evaluation.json"))
    result["evaluation_exit_code"] = evaluated.returncode
    write_json(out / "execution.json", result)
    make_html(out, manifest, result, report)
    # 元ファイルと証跡は、外部描画の実行中にも変更されていないことを照合する。
    for artifact in list(manifest["artifacts"].values()) + manifest["evidence"]:
        verify(base, artifact)
    if fontconfig and digest(Path(str(fontconfig["path"]))) != fontconfig["sha256"]:
        raise ValueError("Font configuration changed during rendering")
    if digest(evaluation_config) != config_sha256:
        raise ValueError("Evaluation configuration changed during rendering")
    print(out / "index.html")
    return evaluated.returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--xtask", type=Path, required=True)
    args = parser.parse_args()
    return build(args.manifest.resolve(), args.out.resolve(), args.xtask.resolve())


if __name__ == "__main__":
    raise SystemExit(main())
