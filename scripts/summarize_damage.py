#!/usr/bin/env python3
"""Summarize a completed damage benchmark without turning missing data into zero."""

import argparse
import csv
import hashlib
import json
from pathlib import Path
import re
import statistics


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--warmup", type=float, default=2)
    parser.add_argument("--require-current-sources", action="store_true")
    args = parser.parse_args()
    manifest = json.loads((args.directory / "manifest.json").read_text())
    if manifest.get("exit_code") != 0 or manifest.get("cases") != 54:
        parser.error("benchmark manifest does not establish 54 completed cases")
    changed = [
        name
        for name, digest in {
            **manifest["source_sha256"],
            **manifest["build_input_sha256"],
            **manifest.get("runner_sha256", {}),
        }.items()
        if not Path(name).is_file()
        or hashlib.sha256(Path(name).read_bytes()).hexdigest() != digest
    ]
    if args.require_current_sources and changed:
        parser.error(
            "benchmark inputs differ from current files: " + ", ".join(changed)
        )
    rows = []
    for backend, transport in (("wayland", "shm"), ("x11", "shm"), ("x11", "putimage")):
        for size in ("640x360", "1920x1080", "3840x2160"):
            for scene in ("counter", "moving", "full"):
                for mode in ("full", "partial"):
                    name = f"{backend}-{transport}-{size}-{scene}-{mode}"
                    log = (args.directory / f"{name}.log").read_text().splitlines()
                    counters = dict(re.findall(r"(\w+)=([\d.]+)", log[-1]))
                    if counters.get("presentation_pauses") != "0":
                        raise RuntimeError(f"{name}: presentation paused")
                    with (args.directory / f"{name}-process.tsv").open() as stream:
                        samples = [
                            row
                            for row in csv.DictReader(stream, delimiter="\t")
                            if float(row["elapsed_s"]) >= args.warmup and row["cpu_s"]
                        ]
                    if len(samples) < 2:
                        raise RuntimeError(f"{name}: insufficient process samples")
                    interval = float(samples[-1]["elapsed_s"]) - float(
                        samples[0]["elapsed_s"]
                    )
                    cpu = (
                        float(samples[-1]["cpu_s"]) - float(samples[0]["cpu_s"])
                    ) / interval
                    fps = float(counters["submitted_per_second"])
                    with (args.directory / f"{name}-frames.tsv").open() as stream:
                        latencies = [
                            int(row["submission_latency_us"])
                            for row in csv.DictReader(stream, delimiter="\t")
                            if int(row["elapsed_ms"]) >= args.warmup * 1000
                        ]
                    if not latencies or fps <= 0:
                        raise RuntimeError(f"{name}: no usable frames")
                    rss_samples = [int(s["rss_kib"]) for s in samples if s["rss_kib"]]
                    if not rss_samples:
                        raise RuntimeError(f"{name}: no usable RSS samples")
                    rows.append(
                        {
                            "backend": backend,
                            "transport": transport,
                            "size": size,
                            "scene": scene,
                            "mode": mode,
                            "submitted_per_second": fps,
                            "cpu_percent_one_core": round(cpu * 100, 3),
                            "cpu_ms_per_submission": round(cpu * 1000 / fps, 3),
                            "rss_max_mib": round(max(rss_samples) / 1024, 3),
                            "submission_us_median": statistics.median(latencies),
                            "submission_us_p95": round(
                                statistics.quantiles(
                                    latencies, n=20, method="inclusive"
                                )[18],
                                1,
                            ),
                            "submitted": int(counters["submitted"]),
                            "presentation_pauses": 0,
                            "process_samples": len(samples),
                        }
                    )
    report = {
        "cases": len(rows),
        "warmup_seconds": args.warmup,
        "current_source_hashes_match": not changed,
        "changed_inputs": changed,
        "cpu_note": "CPU time belongs to the client; compositor and relay are excluded.",
        "rows": rows,
    }
    (args.directory / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    with (args.directory / "summary.tsv").open("w") as stream:
        writer = csv.DictWriter(stream, fieldnames=rows[0].keys(), delimiter="\t")
        writer.writeheader()
        writer.writerows(rows)
    print(f"Summarized {len(rows)} cases; source hashes match: {not changed}")


if __name__ == "__main__":
    main()
