# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Combine comparable benchmark batches without publishing private paths or logs."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import cast

from conversion_benchmark import Trial, digest, summarize, write_json


def combine(batches: list[Path]) -> dict[str, object]:
    resolved = [p.resolve() for p in batches]
    if not resolved or len(set(resolved)) != len(resolved):
        raise ValueError("At least one distinct batch is required; duplicate batches are not samples")
    all_trials: list[Trial] = []
    sources: list[dict[str, object]] = []
    identity: object = None
    reference: dict[str, object] = {}
    repetitions = 0
    excluded = 0
    for folder in resolved:
        environment = json.loads((folder / "environment.json").read_text())
        summary = json.loads((folder / "summary.json").read_text())
        records = json.loads((folder / "records.json").read_text())
        current = (environment["manifest_sha256"],environment["script_sha256"],environment["hardware"],environment["initial"]["system"],environment["initial"]["machine"])
        if identity is None:
            identity, reference = current, environment
        elif current != identity:
            raise ValueError("Batches have different inputs, scripts, hardware, or operating systems")
        count = environment["manifest"]["repetitions"]
        trials = cast(list[Trial],summary["trials"])
        if summary["complete"] is not True or summarize(trials,count) != summary["conditions"]:
            raise ValueError("Stored summary is incomplete or differs from its samples")
        actual_records = [{key:record[key] for key in Trial.__annotations__} for record in records if record["condition"] != "warmup"]
        if actual_records != trials:
            raise ValueError("Raw records differ from the stored samples")
        for record in records:
            if record["condition"] == "warmup":
                if not record["valid"]:
                    raise ValueError("An excluded preparation run failed")
                excluded += 1
        for trial in trials:
            all_trials.append({**trial,"repetition":trial["repetition"]+repetitions})
        repetitions += count
        sources.append({"name":folder.name,"summary_sha256":digest(folder/"summary.json"),"records_sha256":digest(folder/"records.json"),"environment_sha256":digest(folder/"environment.json"),"repetitions_per_group":count})
    manifest = cast(dict[str, object],reference["manifest"])
    variants = cast(dict[str, dict[str, object]],manifest["variants"])
    initial = cast(dict[str, object],reference["initial"])
    return {"complete":True,"sample_count":len(all_trials),"repetitions_per_group":repetitions,"excluded_preparation_runs":excluded,
        "manifest_sha256":reference["manifest_sha256"],"measurement_script_sha256":reference["script_sha256"],
        "revisions":{name:v["commit"] for name,v in variants.items()},
        "binary_sha256":{name:cast(dict[str, object],v["binary"])["sha256"] for name,v in variants.items()},
        "reference_sha256":{name:cast(dict[str, object],v["reference"])["sha256"] for name,v in variants.items()},
        "input_sha256":cast(dict[str, object],manifest["source"])["sha256"],
        "model_sha256":cast(dict[str, object],manifest["model"])["sha256"],
        "font_sha256":[f["sha256"] for f in cast(list[dict[str, object]],manifest["fonts"])],
        "arguments":manifest["arguments"],"platform":{key:initial[key] for key in ["system","machine","python"] if key in initial},
        "hardware":reference["hardware"],"measurement_scope":reference["measurement_scope"],"memory_scope":reference["memory_scope"],
        "uncontrolled":{"os_file_cache":reference["os_file_cache"],"other_caches":reference["other_caches"],"app_nap":reference["app_nap"]},
        "batches":sources,"conditions":summarize(all_trials,repetitions),"trials":all_trials}


def main() -> None:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--batch",type=Path,action="append",required=True)
    parser.add_argument("--out",type=Path,required=True)
    args=parser.parse_args()
    write_json(args.out,combine(args.batch))
    print(args.out)


if __name__=="__main__":
    main()
