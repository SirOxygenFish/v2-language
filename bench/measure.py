"""Wall-time medians of the benchmarks: python bench/measure.py V2_BINARY [RUNS] [NAME ...]

Each runs RUNS times (default 5) at its full size; the median and the fastest run are printed.
The engine benchmarks (first_*, both, py_calls) need Python and Node.js. For instruction counts,
which a 1% change shows, use bench/callgrind.sh on Linux.
"""
import os
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
V2 = os.path.abspath(sys.argv[1])
RUNS = int(sys.argv[2]) if len(sys.argv) > 2 else 5
ALL = ["fib", "loop", "while", "calls", "closures", "lists", "dicts", "strings", "errors",
       "empty", "first_py", "first_js", "first_ts", "both", "py_calls"]
names = sys.argv[3:] or ALL
for name in names:
    times = []
    for _ in range(RUNS):
        start = time.perf_counter()
        p = subprocess.run([V2, name + ".v2"], cwd=HERE, capture_output=True)
        times.append(time.perf_counter() - start)
        if p.returncode != 0:
            sys.exit(f"{name}: exit {p.returncode}: {p.stderr.decode(errors='replace')}")
    print(f"{name:10} median {statistics.median(times) * 1000:8.1f} ms   fastest {min(times) * 1000:8.1f} ms", flush=True)
