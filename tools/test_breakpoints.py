r"""Breakpoint DSL tests for Ferrotap.
Run: python D:\Projects\Ferrotap\tools\test_breakpoints.py

Runs the Rust unit tests that cover the mitmproxy-style filter DSL parser (bpdsl) and the
breakpoint rule / pause-resume-abort-timeout logic (model::breakpoint_tests), then asserts that
each expected test name actually ran and passed (so a silently-missing test fails the suite).
"""
import re, subprocess, sys

ROOT = r"D:\Projects\Ferrotap"

# Tests that must exist and pass: (cargo filter, [required test names])
SUITES = [
    ("bpdsl", [
        "bpdsl::tests::host_extraction",
    ]),
    ("breakpoint_tests", [
        "model::breakpoint_tests::status_specs",
        "model::breakpoint_tests::rule_matching",
        "model::breakpoint_tests::pause_resume_abort_timeout",
    ]),
]

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + (("  -- " + detail[:300]) if detail else ""), flush=True)


def run_suite(flt):
    p = subprocess.run(["cargo", "test", "--lib", flt, "--", "--test-threads=1"],
                       cwd=ROOT, capture_output=True, text=True)
    return p.returncode, p.stdout + "\n" + p.stderr


def main():
    for flt, required in SUITES:
        rc, out = run_suite(flt)
        status = dict((m.group(1), m.group(2)) for m in re.finditer(r"^test (\S+) \.\.\. (\w+)", out, re.M))
        check(f"cargo test --lib {flt} exit code 0", rc == 0, out[-300:] if rc else "")
        selected = [n for n in status if flt in n]
        check(f"{flt}: at least one test ran", len(selected) > 0, str(len(selected)))
        for n in selected:
            check(f"{n}", status[n] == "ok", status[n])
        for n in required:
            check(f"{flt}: required test present: {n}", n in status)
    ok = all(results)
    print(f"\n{sum(results)}/{len(results)} checks passed")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
