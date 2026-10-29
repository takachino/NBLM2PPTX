from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from conversion_benchmark import Trial, summarize, write_json
from summarize_conversion_benchmark import combine


def batch(root: Path, name: str, offset: int = 0) -> Path:
    folder=root/name
    folder.mkdir()
    trials: list[Trial]=[]
    for condition in ["empty","ready"]:
        for variant in ["before","after"]:
            for repetition in range(1,4):
                trials.append({"condition":condition,"variant":variant,"repetition":repetition,"valid":True,
                    "measurement":{"elapsed_seconds":float(10+repetition+offset-(2 if variant=="after" else 0)),"max_rss_bytes":1000,"user_seconds":1.0,"system_seconds":0.1,"exit_code":0,"pid":repetition}})
    write_json(folder/"summary.json",{"complete":True,"trials":trials,"conditions":summarize(trials,3)})
    write_json(folder/"records.json",trials)
    manifest={"repetitions":3,"source":{"sha256":"input","path":"/private/source.pdf"},"model":{"sha256":"model","bytes":208},"fonts":[{"sha256":"font"}],"arguments":["--dpi","150"],
        "variants":{name:{"commit":name,"binary":{"sha256":name},"reference":{"sha256":name}} for name in ["before","after"]},"preparation":{"rustc":{"stdout":"rustc"}}}
    write_json(folder/"environment.json",{"manifest_sha256":"same manifest","script_sha256":"same script","manifest":manifest,"initial":{"system":"macOS","machine":"arm64","processes":{"stdout":"/private/process-list"}},"hardware":{"stdout":"Apple M5"},"measurement_scope":"CLI","memory_scope":"RSS","os_file_cache":"uncontrolled","other_caches":"uncontrolled","app_nap":"uncontrolled"})
    return folder


class BatchSummaryTests(unittest.TestCase):
    def test_all_samples_are_kept_and_private_paths_are_not_published(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            result=combine([batch(root,"one"),batch(root,"two",3)])
            self.assertEqual(result["sample_count"],24)
            self.assertEqual(result["repetitions_per_group"],6)
            self.assertEqual(result["conditions"]["ready"]["before"]["elapsed_seconds"]["median"],13.5)
            self.assertNotIn("/private/source.pdf",json.dumps(result))
            self.assertNotIn("/private/process-list",json.dumps(result))

    def test_different_manifest_script_hardware_and_duplicate_batches_are_rejected(self) -> None:
        for field in ["manifest_sha256","script_sha256","hardware"]:
            with self.subTest(field=field),tempfile.TemporaryDirectory() as temp:
                root=Path(temp);one=batch(root,"one");two=batch(root,"two")
                p=two/"environment.json";value=json.loads(p.read_text());value[field]="different";write_json(p,value)
                with self.assertRaises(ValueError):combine([one,two])
        with tempfile.TemporaryDirectory() as temp:
            one=batch(Path(temp),"one")
            with self.assertRaises(ValueError):combine([one,one])

    def test_a_stored_summary_must_agree_with_its_raw_records(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            one=batch(Path(temp),"one");p=one/"records.json";records=json.loads(p.read_text());records[0]["measurement"]["elapsed_seconds"]=999;write_json(p,records)
            with self.assertRaises(ValueError):combine([one])


if __name__=="__main__":unittest.main()
