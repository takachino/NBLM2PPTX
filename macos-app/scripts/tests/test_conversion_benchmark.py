from __future__ import annotations

import os
from pathlib import Path
import sys
import tempfile
import unittest
import zipfile

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from conversion_benchmark import Trial, measure_process, rss_bytes, schedule, summarize, validate_pptx


def trial(condition: str, variant: str, repetition: int, seconds: float, rss: int) -> Trial:
    return {"condition":condition,"variant":variant,"repetition":repetition,"measurement":{"elapsed_seconds":seconds,"max_rss_bytes":rss,"user_seconds":1.0,"system_seconds":0.1,"exit_code":0,"pid":100+repetition},"valid":True}


class BenchmarkTests(unittest.TestCase):
    def trials(self) -> list[Trial]:
        return [trial(condition,variant,repetition,float(10+repetition if variant=="before" else 8+repetition),1000 if variant=="before" else 1100) for condition in ["empty","ready"] for variant in ["before","after"] for repetition in range(1,4)]

    def test_rss_units_are_normalized_by_platform(self) -> None:
        self.assertEqual(rss_bytes(1024,"Darwin"),1024)
        self.assertEqual(rss_bytes(1024,"Linux"),1048576)
        with self.assertRaises(ValueError):rss_bytes(1024,"Windows")

    def test_each_cache_condition_has_its_own_median_range_and_change(self) -> None:
        result=summarize(self.trials(),3)
        for condition in ["empty","ready"]:
            self.assertEqual(result[condition]["before"]["elapsed_seconds"]["median"],12.0)
            self.assertEqual(result[condition]["after"]["elapsed_seconds"]["min"],9.0)
            self.assertEqual(result[condition]["after"]["elapsed_seconds"]["max"],11.0)
            self.assertAlmostEqual(result[condition]["change_percent"]["elapsed_seconds"],-100/6)
            self.assertAlmostEqual(result[condition]["change_percent"]["max_rss_bytes"],10.0)

    def test_failed_invalid_missing_and_duplicate_trials_cannot_be_reported_as_success(self) -> None:
        for failure in ["exit","invalid","missing","duplicate","nan"]:
            trials=self.trials()
            if failure=="exit":trials[0]["measurement"]["exit_code"]=1
            if failure=="invalid":trials[0]["valid"]=False
            if failure=="missing":trials.pop()
            if failure=="duplicate":trials[1]["repetition"]=1
            if failure=="nan":trials[0]["measurement"]["elapsed_seconds"]=float("nan")
            with self.subTest(failure=failure),self.assertRaises(ValueError):summarize(trials,3)

    def test_schedule_balances_first_position_and_keeps_every_trial(self) -> None:
        result=schedule(3)
        self.assertEqual(len(result),12)
        self.assertEqual(len(set(result)),12)
        self.assertEqual(result[:4],[("empty","before",1),("empty","after",1),("ready","after",1),("ready","before",1)])
        self.assertEqual([variant for _,variant,_ in result[::2]].count("before"),3)

    @unittest.skipUnless(hasattr(os,"wait4"),"wait4 is required")
    def test_rss_is_per_child_and_does_not_inherit_the_previous_peak(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            big=measure_process([sys.executable,"-c","data=bytearray(128*1024*1024); print(len(data))"],dict(os.environ),root/"big")
            small=measure_process([sys.executable,"-c","print('small')"],dict(os.environ),root/"small")
            self.assertEqual(big.exit_code,0)
            self.assertEqual(small.exit_code,0)
            self.assertGreater(big.max_rss_bytes,100*1024*1024)
            self.assertLess(small.max_rss_bytes,big.max_rss_bytes/2)
            self.assertNotEqual(big.pid,small.pid)
            self.assertIn("small",(root/"small/stdout.log").read_text())
            self.assertGreater(small.elapsed_seconds,0)

    def test_nonzero_exit_is_retained(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            result=measure_process([sys.executable,"-c","raise SystemExit(7)"],dict(os.environ),Path(temp)/"failed")
            self.assertEqual(result.exit_code,7)

    def test_changed_pptx_content_is_rejected_even_when_page_and_shape_counts_match(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            for name,text in [("expected","correct"),("actual","wrong")]:
                with zipfile.ZipFile(root/f"{name}.pptx","w") as archive:
                    archive.writestr("ppt/presentation.xml",'<p:presentation xmlns:p="urn:p"><p:sldIdLst><p:sldId id="1"/></p:sldIdLst></p:presentation>')
                    archive.writestr("ppt/slides/slide1.xml",f'<p:sld xmlns:p="urn:p" xmlns:a="urn:a"><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:sld>')
            result=validate_pptx(root/"actual.pptx",root/"expected.pptx")
            self.assertFalse(result["valid"])
            self.assertIn("ppt/slides/slide1.xml",result["differing_parts"])


if __name__ == "__main__":unittest.main()
