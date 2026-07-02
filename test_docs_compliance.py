#!/usr/bin/env python3
"""
V2 Language — Documentation Compliance Test Suite
==================================================
Tests EVERY feature documented in DOCS.md against the actual implementation.
Runs each test as a separate V2 program and checks the output.

Usage:
    python test_docs_compliance.py [--section PATTERN] [--verbose] [--compile] [--keep]
"""

import subprocess, sys, os, re, json, tempfile, textwrap, time, argparse
from pathlib import Path
from dataclasses import dataclass, field
from typing import Optional

V2 = str(Path(__file__).parent / "v2" / "target" / "debug" / "v2.exe")
TMPDIR = Path(__file__).parent / "_test_tmp"

@dataclass
class Test:
    section: str
    name: str
    code: str
    expected: Optional[str] = None
    expect_contains: Optional[list] = None
    expect_not_contains: Optional[list] = None
    expect_error: bool = False
    expect_parse_ok: bool = True
    test_mode: bool = False
    compile_mode: bool = False

@dataclass
class Result:
    test: Test
    passed: bool
    stdout: str = ""
    stderr: str = ""
    exit_code: int = 0
    elapsed_ms: float = 0
    error_msg: str = ""

class TestSuite:
    def __init__(self):
        self.tests: list[Test] = []
    
    def add(self, section: str, name: str, code: str, **kwargs):
        self.tests.append(Test(section=section, name=name, code=textwrap.dedent(code).strip() + "\n", **kwargs))
    
    def run_all(self, filter_pattern=None, verbose=False, also_compile=False, keep=False) -> list[Result]:
        TMPDIR.mkdir(exist_ok=True)
        results = []
        filtered = self.tests
        if filter_pattern:
            pat = re.compile(filter_pattern, re.IGNORECASE)
            filtered = [t for t in self.tests if pat.search(t.section) or pat.search(t.name)]
        
        sections = {}
        for t in filtered:
            sections.setdefault(t.section, []).append(t)
        
        total = len(filtered)
        print(f"\n{'='*70}")
        print(f" V2 Documentation Compliance Tests - {total} tests")
        print(f"{'='*70}\n")
        
        passed = 0
        failed = 0
        
        for sec_name, sec_tests in sections.items():
            print(f"  [{sec_name}]")
            for t in sec_tests:
                r = self._run_one(t, verbose, keep)
                results.append(r)
                if r.passed:
                    passed += 1
                else:
                    failed += 1
                sym = "+" if r.passed else "X"
                tag = "PASS" if r.passed else "FAIL"
                line = f"    {sym} {t.name} [{tag}]"
                if not r.passed:
                    line += f" -- {r.error_msg}"
                print(line)
                if verbose and not r.passed:
                    if r.stdout.strip():
                        for l in r.stdout.strip().split("\n")[:8]:
                            print(f"        stdout: {l}")
                    if r.stderr.strip():
                        for l in r.stderr.strip().split("\n")[:8]:
                            print(f"        stderr: {l}")
                
                if also_compile and not t.compile_mode and not t.test_mode and not t.expect_error:
                    ct = Test(section=t.section, name=t.name + " [compile]", code=t.code,
                              compile_mode=True, expect_parse_ok=True)
                    cr = self._run_one(ct, verbose, keep)
                    results.append(cr)
                    if cr.passed: passed += 1
                    else: failed += 1
                    cs = "+" if cr.passed else "X"
                    print(f"    {cs} {ct.name} [{'PASS' if cr.passed else 'FAIL'}]" + (f" -- {cr.error_msg}" if not cr.passed else ""))
            print()
        
        if not keep:
            import shutil
            shutil.rmtree(TMPDIR, ignore_errors=True)
        
        print(f"{'='*70}")
        print(f" Results: {passed} passed, {failed} failed, {passed + failed} total")
        pct = (passed / (passed + failed) * 100) if (passed + failed) > 0 else 0
        print(f" Pass rate: {pct:.1f}%")
        print(f"{'='*70}\n")
        
        report = {
            "total": len(results), "passed": sum(1 for r in results if r.passed),
            "failed": sum(1 for r in results if not r.passed),
            "pass_rate": f"{pct:.1f}%",
            "sections": {}, "failures": []
        }
        for r in results:
            sec = r.test.section
            if sec not in report["sections"]:
                report["sections"][sec] = {"total": 0, "passed": 0, "failed": 0}
            report["sections"][sec]["total"] += 1
            if r.passed:
                report["sections"][sec]["passed"] += 1
            else:
                report["sections"][sec]["failed"] += 1
                report["failures"].append({
                    "section": sec, "name": r.test.name,
                    "error": r.error_msg, "stderr": r.stderr[:500], "exit_code": r.exit_code
                })
        
        report_path = Path(__file__).parent / "test_compliance_report.json"
        with open(report_path, "w") as f:
            json.dump(report, f, indent=2)
        print(f"Report written to {report_path}\n")
        return results
    
    def _run_one(self, t: Test, verbose: bool, keep: bool) -> Result:
        fname = re.sub(r'[^a-zA-Z0-9_]', '_', f"{t.section}_{t.name}")[:80]
        fpath = TMPDIR / f"{fname}.v2"
        fpath.write_text(t.code, encoding="utf-8")
        
        cmd = [V2]
        if t.compile_mode: cmd += ["-c"]
        if t.test_mode: cmd += ["--test"]
        cmd.append(str(fpath))
        
        start = time.perf_counter()
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=15, encoding="utf-8", errors="replace")
            elapsed = (time.perf_counter() - start) * 1000
        except subprocess.TimeoutExpired:
            return Result(test=t, passed=False, error_msg="TIMEOUT (>15s)", elapsed_ms=15000)
        except Exception as e:
            return Result(test=t, passed=False, error_msg=f"RUN ERROR: {e}")
        
        r = Result(test=t, passed=False, stdout=proc.stdout, stderr=proc.stderr,
                   exit_code=proc.returncode, elapsed_ms=elapsed)
        
        if t.expect_error:
            r.passed = proc.returncode != 0
            if not r.passed: r.error_msg = "Expected error but got exit 0"
        elif t.expected is not None:
            actual = proc.stdout.strip()
            expected = t.expected.strip()
            r.passed = actual == expected
            if not r.passed:
                r.error_msg = f"Expected '{expected[:80]}' got '{actual[:80]}'"
        elif t.expect_contains:
            missing = [s for s in t.expect_contains if s not in proc.stdout]
            r.passed = len(missing) == 0 and proc.returncode == 0
            if not r.passed:
                if proc.returncode != 0:
                    err = (proc.stderr or proc.stdout).strip().split("\n")[0][:120]
                    r.error_msg = f"Exit {proc.returncode}: {err}"
                else:
                    r.error_msg = f"Missing: {missing[:3]}"
        elif t.expect_not_contains:
            found = [s for s in t.expect_not_contains if s in proc.stdout]
            r.passed = len(found) == 0 and proc.returncode == 0
            if not r.passed: r.error_msg = f"Unexpected: {found[:3]}"
        elif t.expect_parse_ok:
            r.passed = proc.returncode == 0
            if not r.passed:
                err = (proc.stderr or proc.stdout).strip().split("\n")[0][:120]
                r.error_msg = f"Exit {proc.returncode}: {err}"
        else:
            r.passed = proc.returncode == 0
        
        if t.compile_mode:
            for ext in [".v2c", ".exe"]:
                p = fpath.with_suffix(ext)
                if p.exists(): p.unlink()
        
        return r

# =============================================================================
# TEST DEFINITIONS
# NOTE: V2's print() does NOT add newline. Use println() for newline output.
# Class constructors: func constructor(...) or func init(...) — NOT new(...)
# self is IMPLICIT in class/struct methods — don't declare as param
# Comprehensions: [x for x in range(5)] — no parens around for clause
# No 'loop' keyword — use while (true) { }
# typeof() for type checking, not type()
# Sets use .contains() not .has()
# sorted() is standalone builtin, .sort() is list method
# range() returns Range object; use list(range(n)) to get a list
# Struct instantiation: new StructName(arg1, arg2) — positional args
# =============================================================================

def build_suite() -> TestSuite:
    s = TestSuite()
    
    # =========================================================================
    # 1. COMMENTS
    # =========================================================================
    s.add("Comments", "single-line comment", """
        // This is a comment
        println("ok")
    """, expected="ok")
    
    s.add("Comments", "block comment", """
        /* block comment */
        println("ok")
    """, expected="ok")
    
    s.add("Comments", "nested block comment", """
        /* outer /* inner */ still comment */
        println("ok")
    """, expected="ok")
    
    s.add("Comments", "inline comment", """
        let x = 5 /* inline comment */
        println(x)
    """, expected="5")
    
    # =========================================================================
    # 2. VARIABLES & CONSTANTS
    # =========================================================================
    s.add("Variables", "let binding", """
        let x = 42
        println(x)
    """, expected="42")
    
    s.add("Variables", "const binding", """
        const PI = 3.14
        println(PI)
    """, expected="3.14")
    
    s.add("Variables", "let with type annotation", """
        let x: int = 10
        println(x)
    """, expected="10")
    
    s.add("Variables", "variable shadowing", """
        let x = "outer"
        if (true) {
            let x = "inner"
            println(x)
        }
        println(x)
    """, expected="inner\nouter")
    
    s.add("Variables", "var mutable binding", """
        let x = 10
        x += 5
        println(x)
    """, expected="15")
    
    # =========================================================================
    # 3. DATA TYPES
    # =========================================================================
    s.add("DataTypes", "int literal", """
        println(42)
    """, expected="42")
    
    s.add("DataTypes", "float literal", """
        println(3.14)
    """, expected="3.14")
    
    s.add("DataTypes", "string literal", """
        println("hello")
    """, expected="hello")
    
    s.add("DataTypes", "bool literals", """
        println(true)
        println(false)
    """, expected="true\nfalse")
    
    s.add("DataTypes", "null literal", """
        println(null)
    """, expected="null")
    
    s.add("DataTypes", "numeric separator", """
        let x = 1_000_000
        println(x)
    """, expected="1000000")
    
    s.add("DataTypes", "typeof builtin", """
        println(typeof(42))
        println(typeof(3.14))
        println(typeof("hi"))
        println(typeof(true))
        println(typeof(null))
    """, expect_contains=["int", "float", "str", "bool", "null"])
    
    s.add("DataTypes", "type conversions", """
        println(int("42"))
        println(float("3.14"))
        println(str(42))
        println(bool(1))
        println(bool(0))
    """, expected="42\n3.14\n42\ntrue\nfalse")
    
    # =========================================================================
    # 4. OPERATORS
    # =========================================================================
    s.add("Operators", "arithmetic", """
        println(2 + 3)
        println(10 - 4)
        println(3 * 5)
        println(10 % 3)
        println(2 ** 8)
        println(7 // 2)
    """, expected="5\n6\n15\n1\n256\n3")
    
    s.add("Operators", "comparison", """
        println(1 == 1)
        println(1 != 2)
        println(1 < 2)
        println(2 > 1)
        println(1 <= 1)
        println(2 >= 2)
    """, expected="true\ntrue\ntrue\ntrue\ntrue\ntrue")
    
    s.add("Operators", "logical and/or/not", """
        println(true and true)
        println(true and false)
        println(false or true)
        println(not true)
    """, expected="true\nfalse\ntrue\nfalse")
    
    s.add("Operators", "bitwise", """
        println(5 & 3)
        println(5 | 3)
        println(5 ^ 3)
        println(~0)
        println(1 << 4)
        println(16 >> 2)
    """, expected="1\n7\n6\n-1\n16\n4")
    
    s.add("Operators", "assignment operators", """
        let x = 10
        x += 5
        println(x)
        x -= 3
        println(x)
        x *= 2
        println(x)
        x //= 3
        println(x)
    """, expected="15\n12\n24\n8")
    
    s.add("Operators", "increment/decrement", """
        let x = 5
        x++
        println(x)
        x--
        println(x)
    """, expected="6\n5")
    
    s.add("Operators", "ternary operator", """
        let x = true ? "yes" : "no"
        println(x)
        let y = false ? "yes" : "no"
        println(y)
    """, expected="yes\nno")
    
    s.add("Operators", "null coalescing", """
        let x = null ?? "default"
        println(x)
        let y = "value" ?? "default"
        println(y)
    """, expected="default\nvalue")
    
    s.add("Operators", "in operator", """
        println(2 in [1, 2, 3])
        println(5 in [1, 2, 3])
        println("a" in "apple")
    """, expected="true\nfalse\ntrue")
    
    s.add("Operators", "not in operator", """
        println(5 not in [1, 2, 3])
        println(2 not in [1, 2, 3])
    """, expected="true\nfalse")
    
    s.add("Operators", "is operator", """
        println(42 is int)
        println("hi" is str)
        println(3.14 is float)
    """, expected="true\ntrue\ntrue")
    
    # =========================================================================
    # 5. STRINGS
    # =========================================================================
    s.add("Strings", "basic string ops", """
        let s = "hello"
        println(s.len())
        println(s.upper())
        println(s.lower())
    """, expected="5\nHELLO\nhello")
    
    s.add("Strings", "f-string interpolation", """
        let name = "World"
        println(f"Hello ${name}")
    """, expected="Hello World")
    
    s.add("Strings", "f-string expression", """
        let x = 5
        println(f"${x + 1}")
    """, expected="6")
    
    s.add("Strings", "string methods - trim/split", """
        println("  hello  ".trim())
        let parts = "a,b,c".split(",")
        println(parts)
    """, expect_contains=["hello", "a", "b", "c"])
    
    s.add("Strings", "string methods - contains/starts_with/ends_with", """
        println("hello world".contains("world"))
        println("hello".starts_with("hel"))
        println("hello".ends_with("llo"))
    """, expected="true\ntrue\ntrue")
    
    s.add("Strings", "string methods - replace", """
        println("hello world".replace("world", "V2"))
    """, expected="hello V2")
    
    s.add("Strings", "string indexing", """
        let s = "hello"
        println(s[0])
        println(s[-1])
    """, expected="h\no")
    
    s.add("Strings", "string slicing", """
        let s = "hello"
        println(s[1:4])
    """, expected="ell")
    
    s.add("Strings", "string concatenation", """
        println("hello" + " " + "world")
    """, expected="hello world")
    
    s.add("Strings", "string repetition", """
        println("ab".repeat(3))
    """, expected="ababab")
    
    s.add("Strings", "raw string", r"""
        let s = r"no\nescape"
        println(s)
    """, expect_contains=["no\\nescape"])
    
    s.add("Strings", "triple-quoted string", '''
        let s = """line1
line2"""
        println(s)
    ''', expect_contains=["line1", "line2"])
    
    s.add("Strings", "string methods - reverse", """
        println("hello".reverse())
    """, expected="olleh")
    
    s.add("Strings", "string capitalize/title", """
        println("hello world".capitalize())
        println("hello world".title())
    """, expected="Hello world\nHello World")
    
    s.add("Strings", "chr and ord builtins", """
        println(chr(65))
        println(ord("A"))
    """, expected="A\n65")
    
    s.add("Strings", "string center", """
        println("hi".center(6, "-"))
    """, expected="--hi--")
    
    s.add("Strings", "string isdigit/isalpha", """
        println("123".isdigit())
        println("abc".isalpha())
        println("12a".isdigit())
    """, expected="true\ntrue\nfalse")
    
    s.add("Strings", "string swapcase", """
        println("Hello World".swapcase())
    """, expected="hELLO wORLD")
    
    s.add("Strings", "string char_at", """
        println("hello".char_at(1))
    """, expected="e")
    
    # =========================================================================
    # 6. LISTS
    # =========================================================================
    s.add("Lists", "list literal and indexing", """
        let a = [1, 2, 3]
        println(a[0])
        println(a[-1])
        println(len(a))
    """, expected="1\n3\n3")
    
    s.add("Lists", "push and pop", """
        let a = [1, 2, 3]
        a.push(4)
        println(a)
        let v = a.pop()
        println(v)
        println(a)
    """, expect_contains=["4", "3"])
    
    s.add("Lists", "list slicing", """
        let a = [0, 1, 2, 3, 4]
        println(a[1:4])
    """, expect_contains=["1", "2", "3"])
    
    s.add("Lists", "list comprehension", """
        let squares = [x * x for x in range(5)]
        println(squares)
    """, expect_contains=["0", "1", "4", "9", "16"])
    
    s.add("Lists", "list comprehension with filter", """
        let evens = [x for x in range(10) if x % 2 == 0]
        println(evens)
    """, expect_contains=["0", "2", "4", "6", "8"])
    
    s.add("Lists", "map/filter/reduce", """
        let a = [1, 2, 3, 4, 5]
        let doubled = a.map(lambda(x) => x * 2)
        println(doubled)
        let evens = a.filter(lambda(x) => x % 2 == 0)
        println(evens)
        let total = a.reduce(lambda(acc, x) => acc + x, 0)
        println(total)
    """, expect_contains=["2", "4", "6", "8", "10", "15"])
    
    s.add("Lists", "sort method", """
        let a = [3, 1, 2]
        println(a.sort())
    """, expect_contains=["1", "2", "3"])
    
    s.add("Lists", "sorted builtin", """
        println(sorted([3, 1, 2]))
    """, expect_contains=["1", "2", "3"])
    
    s.add("Lists", "reversed builtin", """
        println(reversed([1, 2, 3]))
    """, expect_contains=["3", "2", "1"])
    
    s.add("Lists", "contains and index_of", """
        let a = [10, 20, 30]
        println(a.contains(20))
        println(a.index_of(30))
    """, expected="true\n2")
    
    s.add("Lists", "any/all/sum", """
        println([1, 2, 3].any(lambda(x) => x > 2))
        println([1, 2, 3].all(lambda(x) => x > 0))
        println([1, 2, 3].sum())
    """, expected="true\ntrue\n6")
    
    s.add("Lists", "enumerate and zip", """
        let a = ["a", "b", "c"]
        for (pair in enumerate(a)) {
            println(pair)
        }
    """, expect_contains=["0", "a", "1", "b", "2", "c"])
    
    s.add("Lists", "flatten", """
        let a = [[1, 2], [3, 4]]
        println(a.flatten())
    """, expect_contains=["1", "2", "3", "4"])
    
    s.add("Lists", "first/last/is_empty", """
        let a = [10, 20, 30]
        println(a.first())
        println(a.last())
        println(a.is_empty())
        println([].is_empty())
    """, expected="10\n30\nfalse\ntrue")
    
    s.add("Lists", "unique", """
        let a = [1, 2, 2, 3, 3, 3]
        println(a.unique())
    """, expect_contains=["1", "2", "3"])
    
    s.add("Lists", "join", """
        println(["a", "b", "c"].join("-"))
    """, expected="a-b-c")
    
    s.add("Lists", "list count", """
        println([1, 2, 2, 3, 2].count(2))
    """, expected="3")
    
    s.add("Lists", "list each", """
        [1, 2, 3].each(lambda(x) => println(x))
    """, expected="1\n2\n3")
    
    s.add("Lists", "list find", """
        let r = [1, 2, 3, 4].find(lambda(x) => x > 2)
        println(r)
    """, expect_contains=["3"])
    
    s.add("Lists", "list insert", """
        let a = [1, 2, 3]
        a.insert(1, 10)
        println(a)
    """, expect_contains=["1", "10", "2", "3"])
    
    s.add("Lists", "list take", """
        println([1, 2, 3, 4, 5].take(3))
    """, expect_contains=["1", "2", "3"])
    
    # =========================================================================
    # 7. DICTIONARIES
    # =========================================================================
    s.add("Dicts", "dict literal and access", """
        let d = {"name": "Alice", "age": 30}
        println(d["name"])
        println(d["age"])
    """, expected="Alice\n30")
    
    s.add("Dicts", "dict methods - keys/values", """
        let d = {"a": 1, "b": 2}
        println(d.keys())
        println(d.values())
    """, expect_contains=["a", "b", "1", "2"])
    
    s.add("Dicts", "dict methods - has/get/set/remove", """
        let d = {"x": 10}
        println(d.has("x"))
        println(d.get("y", 0))
        d.set("y", 20)
        println(d["y"])
        d.remove("x")
        println(d.has("x"))
    """, expected="true\n0\n20\nfalse")
    
    s.add("Dicts", "dict len and is_empty", """
        let d = {"a": 1}
        println(d.len())
        println(d.is_empty())
        println({}.is_empty())
    """, expected="1\nfalse\ntrue")
    
    s.add("Dicts", "dict comprehension", """
        let d = {str(x): x * x for x in range(4)}
        println(d)
    """, expect_contains=["0", "1", "4", "9"])
    
    s.add("Dicts", "dict merge", """
        let a = {"x": 1}
        let b = {"y": 2}
        let c = a.merge(b)
        println(c)
    """, expect_contains=["x", "y", "1", "2"])
    
    s.add("Dicts", "dict items", """
        let d = {"a": 1, "b": 2}
        for (pair in d.items()) {
            println(pair)
        }
    """, expect_contains=["a", "b", "1", "2"])
    
    s.add("Dicts", "dict filter", """
        let d = {"a": 1, "b": 2, "c": 3}
        let big = d.filter(lambda(k, v) => v > 1)
        println(big)
    """, expect_contains=["b", "c", "2", "3"])
    
    # =========================================================================
    # 8. TUPLES
    # =========================================================================
    s.add("Tuples", "tuple literal and indexing", """
        let t = (1, "hello", true)
        println(t[0])
        println(t[1])
        println(t[2])
    """, expected="1\nhello\ntrue")
    
    s.add("Tuples", "tuple destructuring", """
        let (a, b, c) = (10, 20, 30)
        println(a)
        println(b)
        println(c)
    """, expected="10\n20\n30")
    
    # =========================================================================
    # 9. SETS
    # =========================================================================
    s.add("Sets", "set literal", """
        let s = #{1, 2, 3}
        println(s.len())
        println(s.contains(2))
        println(s.contains(5))
    """, expected="3\ntrue\nfalse")
    
    s.add("Sets", "set add/remove", """
        let s = #{1, 2}
        s.add(3)
        println(s.contains(3))
        s.remove(1)
        println(s.contains(1))
    """, expected="true\nfalse")
    
    s.add("Sets", "set union/intersect/difference", """
        let a = #{1, 2, 3}
        let b = #{2, 3, 4}
        let u = a.union(b)
        println(u.len())
        let i = a.intersect(b)
        println(i.len())
        let d = a.difference(b)
        println(d.len())
    """, expected="4\n2\n1")
    
    s.add("Sets", "set comprehension", """
        let s = #{x * 2 for x in range(5)}
        println(s.len())
    """, expected="5")
    
    s.add("Sets", "set is_subset/is_superset", """
        let a = #{1, 2}
        let b = #{1, 2, 3}
        println(a.is_subset(b))
        println(b.is_superset(a))
    """, expected="true\ntrue")
    
    # =========================================================================
    # 10. CONTROL FLOW
    # =========================================================================
    s.add("ControlFlow", "if/elif/else", """
        let x = 15
        if (x > 20) {
            println("big")
        } elif (x > 10) {
            println("medium")
        } else {
            println("small")
        }
    """, expected="medium")
    
    s.add("ControlFlow", "for-in loop", """
        for (i in [1, 2, 3]) {
            println(i)
        }
    """, expected="1\n2\n3")
    
    s.add("ControlFlow", "for-in range", """
        for (i in 0..5) {
            println(i)
        }
    """, expected="0\n1\n2\n3\n4")
    
    s.add("ControlFlow", "for-in inclusive range", """
        for (i in 0..=3) {
            println(i)
        }
    """, expected="0\n1\n2\n3")
    
    s.add("ControlFlow", "while loop", """
        let i = 0
        while (i < 3) {
            println(i)
            i += 1
        }
    """, expected="0\n1\n2")
    
    s.add("ControlFlow", "while true with break", """
        let i = 0
        while (true) {
            if (i >= 3) { break }
            println(i)
            i += 1
        }
    """, expected="0\n1\n2")
    
    s.add("ControlFlow", "continue", """
        for (i in 0..5) {
            if (i % 2 == 0) { continue }
            println(i)
        }
    """, expected="1\n3")
    
    s.add("ControlFlow", "C-style for loop", """
        for (let i = 0; i < 3; i += 1) {
            println(i)
        }
    """, expected="0\n1\n2")
    
    s.add("ControlFlow", "labeled break", """
        outer: for (i in 0..3) {
            for (j in 0..3) {
                if (j == 1) { break outer }
                println(f"${i},${j}")
            }
        }
    """, expected="0,0")
    
    s.add("ControlFlow", "match basic", """
        let x = 2
        match (x) {
            case (1) { println("one") }
            case (2) { println("two") }
            case (3) { println("three") }
            default { println("other") }
        }
    """, expected="two")
    
    s.add("ControlFlow", "match with guard", """
        let x = 15
        match (x) {
            case (n) if (n > 10) { println("big") }
            case (n) if (n > 5) { println("medium") }
            default { println("small") }
        }
    """, expected="big")
    
    s.add("ControlFlow", "match or pattern", """
        let x = 2
        match (x) {
            case (1 | 2 | 3) { println("small") }
            default { println("other") }
        }
    """, expected="small")
    
    s.add("ControlFlow", "range pattern in match", """
        let x = 5
        match (x) {
            case (1..=3) { println("low") }
            case (4..=6) { println("mid") }
            case (7..=10) { println("high") }
            default { println("out") }
        }
    """, expected="mid")
    
    # =========================================================================
    # 11. FUNCTIONS
    # =========================================================================
    s.add("Functions", "basic function", """
        func greet(name) {
            return f"Hello ${name}"
        }
        println(greet("World"))
    """, expected="Hello World")
    
    s.add("Functions", "default params", """
        func add(a, b = 10) {
            return a + b
        }
        println(add(5))
        println(add(5, 20))
    """, expected="15\n25")
    
    s.add("Functions", "variadic params", """
        func sum_all(...args) {
            let total = 0
            for (a in args) { total += a }
            return total
        }
        println(sum_all(1, 2, 3, 4))
    """, expected="10")
    
    s.add("Functions", "named arguments", """
        func greet(name, greeting = "Hello") {
            return f"${greeting} ${name}"
        }
        println(greet("Alice", greeting: "Hi"))
    """, expected="Hi Alice")
    
    s.add("Functions", "first-class functions", """
        func apply(f, x) { return f(x) }
        func double(x) { return x * 2 }
        println(apply(double, 5))
    """, expected="10")
    
    s.add("Functions", "recursion", """
        func fib(n) {
            if (n <= 1) { return n }
            return fib(n - 1) + fib(n - 2)
        }
        println(fib(10))
    """, expected="55")
    
    s.add("Functions", "closure", """
        func make_counter() {
            let count = 0
            return lambda() {
                count += 1
                return count
            }
        }
        let c = make_counter()
        println(c())
        println(c())
        println(c())
    """, expected="1\n2\n3")
    
    s.add("Functions", "multiple return values", """
        func swap(a, b) {
            return (b, a)
        }
        let (x, y) = swap(1, 2)
        println(x)
        println(y)
    """, expected="2\n1")
    
    s.add("Functions", "return type annotation", """
        func add(a: int, b: int) -> int {
            return a + b
        }
        println(add(3, 4))
    """, expected="7")
    
    # =========================================================================
    # 12. DEFER
    # =========================================================================
    s.add("Defer", "basic defer", """
        func example() {
            defer { println("cleanup") }
            println("work")
        }
        example()
    """, expected="work\ncleanup")
    
    s.add("Defer", "defer LIFO order", """
        func example() {
            defer { println("first defer") }
            defer { println("second defer") }
            println("body")
        }
        example()
    """, expected="body\nsecond defer\nfirst defer")
    
    # =========================================================================
    # 13. DECORATORS
    # =========================================================================
    s.add("Decorators", "custom decorator", """
        func my_decorator(f) {
            return lambda(...args) {
                println("before")
                let result = f(...args)
                println("after")
                return result
            }
        }
        
        @my_decorator
        func greet(name) {
            println(f"Hello ${name}")
        }
        
        greet("World")
    """, expected="before\nHello World\nafter")
    
    s.add("Decorators", "memo decorator", """
        @memo
        func square(n) {
            return n * n
        }
        println(square(5))
        println(square(5))
    """, expected="25\n25")
    
    # =========================================================================
    # 14. LAMBDAS
    # =========================================================================
    s.add("Lambdas", "arrow lambda", """
        let double = lambda(x) => x * 2
        println(double(5))
    """, expected="10")
    
    s.add("Lambdas", "block lambda", """
        let add = lambda(a, b) {
            return a + b
        }
        println(add(3, 4))
    """, expected="7")
    
    s.add("Lambdas", "lambda as argument", """
        let nums = [1, 2, 3, 4, 5]
        let evens = nums.filter(lambda(x) => x % 2 == 0)
        println(evens)
    """, expect_contains=["2", "4"])
    
    # =========================================================================
    # 15. LAZY EXPRESSIONS
    # =========================================================================
    s.add("Lazy", "lazy evaluation", """
        let x = lazy 42
        println(x)
    """, expected="42")
    
    # =========================================================================
    # 16. CLASSES
    # =========================================================================
    s.add("Classes", "basic class with constructor", """
        class Point {
            func constructor(x, y) {
                self.x = x
                self.y = y
            }
            func to_string() {
                return f"(${self.x}, ${self.y})"
            }
        }
        let p = new Point(3, 4)
        println(p.to_string())
    """, expected="(3, 4)")
    
    s.add("Classes", "inheritance", """
        class Animal {
            func constructor(name) { self.name = name }
            func speak() { return "..." }
        }
        class Dog extends Animal {
            func speak() { return f"${self.name} says Woof!" }
        }
        let d = new Dog("Rex")
        println(d.speak())
    """, expected="Rex says Woof!")
    
    s.add("Classes", "super call", """
        class Base {
            func constructor(x) { self.x = x }
        }
        class Child extends Base {
            func constructor(x, y) {
                super(x)
                self.y = y
            }
        }
        let c = new Child(1, 2)
        println(c.x)
        println(c.y)
    """, expected="1\n2")
    
    s.add("Classes", "computed properties", """
        class Circle {
            func constructor(r) {
                self.r = r
            }
            get area -> float {
                return 3.14 * self.r * self.r
            }
        }
        let c = new Circle(5)
        println(c.area)
    """, expect_contains=["78.5"])
    
    s.add("Classes", "operator overload __add__", """
        class Vec2 {
            func constructor(x, y) {
                self.x = x
                self.y = y
            }
            func __add__(other) {
                return new Vec2(self.x + other.x, self.y + other.y)
            }
            func __str__() {
                return f"(${self.x}, ${self.y})"
            }
        }
        let a = new Vec2(1, 2)
        let b = new Vec2(3, 4)
        let c = a + b
        println(c)
    """, expected="(4, 6)")
    
    s.add("Classes", "operator overload __eq__", """
        class Val {
            func constructor(n) { self.n = n }
            func __eq__(other) {
                return self.n == other.n
            }
        }
        println(new Val(5) == new Val(5))
        println(new Val(5) == new Val(3))
    """, expected="true\nfalse")
    
    # __len__ dispatch from len() — IMPLEMENTED
    s.add("Classes", "operator overload __len__", """
        class MyList {
            func constructor(items) {
                self.items = items
            }
            func __len__() { return len(self.items) }
        }
        let m = new MyList([1, 2, 3])
        println(len(m))
    """, expected="3")
    
    # __getitem__ dispatch from [] — IMPLEMENTED
    s.add("Classes", "operator overload __getitem__", """
        class MyContainer {
            func constructor(data) {
                self.data = data
            }
            func __getitem__(key) {
                return self.data[key]
            }
        }
        let c = new MyContainer([10, 20, 30])
        println(c[1])
    """, expected="20")
    
    # Iterator protocol with iter()/next()/is_done() — IMPLEMENTED
    s.add("Classes", "operator overload __iter__/__next__", """
        class Counter {
            func constructor(max) {
                self.max = max
                self.current = 0
            }
            func iter() { return self }
            func is_done() {
                return self.current >= self.max
            }
            func next() {
                let val = self.current
                self.current += 1
                return val
            }
        }
        for (v in new Counter(3)) {
            println(v)
        }
    """, expected="0\n1\n2")
    
    # __contains__ dispatch from 'in' — IMPLEMENTED
    s.add("Classes", "operator overload __contains__", """
        class Bag {
            func constructor(items) {
                self.items = items
            }
            func __contains__(item) {
                return self.items.contains(item)
            }
        }
        let bag = new Bag([1, 2, 3])
        println(2 in bag)
        println(5 in bag)
    """, expected="true\nfalse")
    
    # __call__ dispatch is NOT in DOCS. Testing direct method call.
    s.add("Classes", "operator overload __call__", """
        class Adder {
            func constructor(n) {
                self.n = n
            }
            func __call__(x) {
                return self.n + x
            }
        }
        let add5 = new Adder(5)
        println(add5.__call__(3))
    """, expected="8")
    
    s.add("Classes", "sealed class parses", """
        sealed class Color {
            Red {}
            Green {}
            Blue {}
        }
        println("parsed ok")
    """, expected="parsed ok")
    
    # =========================================================================
    # 17. STRUCTS
    # =========================================================================
    s.add("Structs", "basic struct", """
        struct Point { x, y }
        let p = new Point(3, 4)
        println(p.x)
        println(p.y)
    """, expected="3\n4")
    
    s.add("Structs", "struct with impl", """
        struct Vec2 { x, y }
        impl Vec2 {
            func length() -> float {
                return sqrt(self.x * self.x + self.y * self.y)
            }
        }
        let v = new Vec2(3.0, 4.0)
        println(v.length())
    """, expected="5.0")
    
    # =========================================================================
    # 18. ENUMS
    # =========================================================================
    s.add("Enums", "basic enum", """
        enum Color { Red, Green, Blue }
        let c = Color.Red
        println(c)
    """, expect_contains=["Red"])
    
    s.add("Enums", "enum with data", """
        enum Shape {
            Circle(r)
            Rect(w, h)
        }
        let s = Shape.Circle(5)
        match (s) {
            case (Shape.Circle(r)) { println(f"Circle r=${r}") }
            case (Shape.Rect(w, h)) { println(f"Rect ${w}x${h}") }
        }
    """, expected="Circle r=5")
    
    # Enum impl — IMPLEMENTED
    s.add("Enums", "enum with impl", """
        enum Direction { Up, Down, Left, Right }
        impl Direction {
            func is_vertical() {
                return self == Direction.Up or self == Direction.Down
            }
        }
        println(Direction.Up.is_vertical())
    """, expected="true")
    
    # =========================================================================
    # 19. TRAITS
    # =========================================================================
    # Trait abstract methods — IMPLEMENTED
    s.add("Traits", "basic trait and impl", """
        trait Greetable {
            func greet() -> str
        }
        struct Person { name }
        impl Greetable for Person {
            func greet() -> str {
                return f"Hi, I'm ${self.name}"
            }
        }
        let p = new Person("Alice")
        println(p.greet())
    """, expected="Hi, I'm Alice")
    
    # Trait default method inheritance — IMPLEMENTED
    s.add("Traits", "default method", """
        trait Describable {
            func describe() -> str {
                return "I am something"
            }
        }
        struct Widget {}
        impl Describable for Widget {}
        let w = new Widget()
        println(w.describe())
    """, expected="I am something")
    
    # =========================================================================
    # 20. GENERICS
    # =========================================================================
    s.add("Generics", "generic function", """
        func identity<T>(x: T) -> T {
            return x
        }
        println(identity(42))
        println(identity("hello"))
    """, expected="42\nhello")
    
    s.add("Generics", "generic struct", """
        struct Box_g<T> { value }
        let b = new Box_g(42)
        println(b.value)
    """, expected="42")
    
    # =========================================================================
    # 21. ERROR HANDLING
    # =========================================================================
    s.add("ErrorHandling", "try/catch/finally", """
        try {
            throw "oops"
        } catch (e) {
            println(f"caught: ${e}")
        } finally {
            println("finally")
        }
    """, expected="caught: oops\nfinally")
    
    s.add("ErrorHandling", "Result Ok/Err", """
        func divide(a, b) {
            if (b == 0) { return Err("division by zero") }
            return Ok(a / b)
        }
        let r = divide(10, 2)
        println(r)
        let e = divide(10, 0)
        println(e)
    """, expect_contains=["Ok", "5", "Err", "division by zero"])
    
    s.add("ErrorHandling", "Result unwrap", """
        let r = Ok(42)
        println(r.unwrap())
    """, expected="42")
    
    s.add("ErrorHandling", "Result map", """
        let r = Ok(5)
        let doubled = r.map(lambda(x) => x * 2)
        println(doubled)
    """, expect_contains=["Ok", "10"])
    
    s.add("ErrorHandling", "Option Some/None", """
        let a = Some(42)
        let b = None
        println(is_some(a))
        println(is_none(b))
        println(unwrap(a))
    """, expected="true\ntrue\n42")
    
    s.add("ErrorHandling", "if let with Option", """
        let val = Some(42)
        if let Some(x) = val {
            println(f"Got ${x}")
        } else {
            println("Nothing")
        }
    """, expected="Got 42")
    
    s.add("ErrorHandling", "let else", """
        let val = Some(42)
        let Some(x) = val else {
            println("was none")
            return
        }
        println(x)
    """, expected="42")
    
    s.add("ErrorHandling", "try_wrap", """
        func might_fail() {
            throw "error!"
        }
        let result = try_wrap(might_fail)
        println(is_err(result))
    """, expected="true")
    
    # =========================================================================
    # 22. GENERATORS
    # =========================================================================
    s.add("Generators", "basic generator", """
        func* count_up(n) {
            for (i in 0..n) {
                yield i
            }
        }
        for (v in count_up(4)) {
            println(v)
        }
    """, expected="0\n1\n2\n3")
    
    s.add("Generators", "generator next()", """
        func* nums() {
            yield 1
            yield 2
            yield 3
        }
        let g = nums()
        println(g.next())
        println(g.next())
        println(g.next())
    """, expect_contains=["1", "2", "3"])
    
    # =========================================================================
    # 23. ASYNC/AWAIT
    # =========================================================================
    s.add("Async", "basic async/await", """
        async func fetch_value() {
            return 42
        }
        let result = await fetch_value()
        println(result)
    """, expected="42")
    
    # =========================================================================
    # 24. MACROS
    # =========================================================================
    s.add("Macros", "basic macro", """
        macro debug!(val) {
            println(f"DEBUG: ${val}")
        }
        debug!(42)
    """, expected="DEBUG: 42")
    
    # =========================================================================
    # 25. COMPTIME
    # =========================================================================
    s.add("Comptime", "comptime block", """
        comptime {
            static_assert(1 + 1 == 2, "math works")
        }
        println("ok")
    """, expected="ok")
    
    s.add("Comptime", "static_assert", """
        static_assert(true, "should pass")
        println("ok")
    """, expected="ok")
    
    # =========================================================================
    # 26. PIPE AND SPREAD
    # =========================================================================
    s.add("PipeSpread", "pipe operator", """
        func double(x) { return x * 2 }
        func add_one(x) { return x + 1 }
        let result = 5 |> double |> add_one
        println(result)
    """, expected="11")
    
    s.add("PipeSpread", "spread in call", """
        func add(a, b, c) { return a + b + c }
        let args = [1, 2, 3]
        println(add(...args))
    """, expected="6")
    
    s.add("PipeSpread", "spread in list", """
        let a = [1, 2]
        let b = [0, ...a, 3]
        println(b)
    """, expect_contains=["0", "1", "2", "3"])
    
    # =========================================================================
    # 27. TYPE ALIAS & NEWTYPE
    # =========================================================================
    s.add("TypeAlias", "type alias", """
        type Num = int
        let x: Num = 42
        println(x)
    """, expected="42")
    
    s.add("TypeAlias", "newtype", """
        newtype UserId = int
        let id: UserId = 42
        println(id)
    """, expected="42")
    
    # =========================================================================
    # 28. DO BLOCK EXPRESSION
    # =========================================================================
    s.add("DoBlock", "do block as expression", """
        let result = do {
            let a = 5
            let b = 10
            a + b
        }
        println(result)
    """, expected="15")
    
    # =========================================================================
    # 29. FOR DESTRUCTURING
    # =========================================================================
    s.add("ForDestructure", "for with tuple destructure", """
        let pairs = [(1, "a"), (2, "b"), (3, "c")]
        for ((n, s) in pairs) {
            println(f"${n}=${s}")
        }
    """, expected="1=a\n2=b\n3=c")
    
    s.add("ForDestructure", "for with list destructure", """
        let items = [[1, 2], [3, 4], [5, 6]]
        for ([a, b] in items) {
            println(a + b)
        }
    """, expected="3\n7\n11")
    
    # =========================================================================
    # 30. RUNTIME INTROSPECTION
    # =========================================================================
    s.add("Introspection", "typeof()", """
        println(typeof(42))
        println(typeof("hi"))
        println(typeof([1, 2]))
    """, expect_contains=["int", "str", "list"])
    
    s.add("Introspection", "callable()", """
        func f() {}
        println(callable(f))
        println(callable(42))
    """, expected="true\nfalse")
    
    # =========================================================================
    # 31. IMPORTS
    # =========================================================================
    s.add("Imports", "import std.math", """
        import "std.math" as m
        println(m.PI > 3.14)
    """, expected="true")
    
    s.add("Imports", "import std.math global", """
        import "std.math"
        println(math.floor(3.7))
    """, expected="3")
    
    # =========================================================================
    # 32. TESTING with assert_eq / assert
    # =========================================================================
    s.add("Testing", "test block with assert_eq", """
        test "basic math" {
            assert_eq(1 + 1, 2)
            assert(true)
        }
    """, expect_contains=["PASS"], test_mode=True)
    
    s.add("Testing", "test block with assert_ne", """
        test "inequality" {
            assert_ne(1, 2)
        }
    """, expect_contains=["PASS"], test_mode=True)
    
    s.add("Testing", "test blocks skipped without --test", """
        test "should not run" {
            println("SHOULD NOT SEE THIS")
        }
        println("ok")
    """, expected="ok")
    
    # =========================================================================
    # 33. BUILTINS
    # =========================================================================
    s.add("Builtins", "abs/min/max", """
        println(abs(-5))
        println(min(3, 7))
        println(max(3, 7))
    """, expected="5\n3\n7")
    
    s.add("Builtins", "round/floor/ceil", """
        println(round(3.6))
        println(floor(3.9))
        println(ceil(3.1))
    """, expect_contains=["4", "3"])
    
    s.add("Builtins", "sqrt/pow", """
        println(sqrt(16))
        println(pow(2, 10))
    """, expect_contains=["4", "1024"])
    
    s.add("Builtins", "range with list()", """
        println(list(range(5)))
        println(list(range(2, 5)))
    """, expect_contains=["0", "1", "2", "3", "4"])
    
    s.add("Builtins", "len", """
        println(len([1, 2, 3]))
        println(len("hello"))
        println(len({"a": 1}))
    """, expected="3\n5\n1")
    
    s.add("Builtins", "enumerate", """
        for (pair in enumerate(["a", "b"])) {
            println(pair)
        }
    """, expect_contains=["0", "a", "1", "b"])
    
    s.add("Builtins", "zip", """
        for (pair in zip([1, 2], ["a", "b"])) {
            println(pair)
        }
    """, expect_contains=["1", "a", "2", "b"])
    
    s.add("Builtins", "sum builtin", """
        println(sum([1, 2, 3, 4, 5]))
    """, expected="15")
    
    s.add("Builtins", "sorted/reversed", """
        println(sorted([3, 1, 2]))
        println(reversed([1, 2, 3]))
    """, expect_contains=["1", "2", "3"])
    
    s.add("Builtins", "clone", """
        let a = [1, 2, 3]
        let b = clone(a)
        b.push(4)
        println(len(a))
        println(len(b))
    """, expected="3\n4")
    
    s.add("Builtins", "hex/bin/oct", """
        println(hex(255))
        println(bin(10))
        println(oct(8))
    """, expect_contains=["ff", "1010", "10"])
    
    s.add("Builtins", "json_parse and json_stringify", """
        let obj = json_parse('{"a": 1, "b": 2}')
        println(obj["a"])
        let s = json_stringify({"x": 10})
        println(s)
    """, expect_contains=["1", "x", "10"])
    
    # freeze() in-place on lists — IMPLEMENTED
    s.add("Builtins", "freeze and is_frozen", """
        let data = [1, 2, 3]
        freeze(data)
        println(is_frozen(data))
    """, expect_contains=["true"])
    
    # =========================================================================
    # 34. FILE I/O BUILTINS
    # =========================================================================
    s.add("FileIO", "write and read file", """
        write_file("_test_tmp_io.txt", "hello v2")
        let content = read_file("_test_tmp_io.txt")
        println(content)
        delete_file("_test_tmp_io.txt")
    """, expected="hello v2")
    
    # =========================================================================
    # 35. COW
    # =========================================================================
    s.add("COW", "copy-on-write class", """
        @cow
        class Data {
            func constructor(val) { self.val = val }
        }
        let a = new Data(10)
        let b = a
        b.val = 20
        println(a.val)
        println(b.val)
    """, expected="10\n20")
    
    # =========================================================================
    # 36. TAIL CALL OPTIMIZATION
    # =========================================================================
    s.add("TCO", "deep recursion with TCO", """
        func count_down(n) {
            if (n == 0) { return 0 }
            return count_down(n - 1)
        }
        println(count_down(50000))
    """, expected="0")
    
    # =========================================================================
    # 37. STD.MATH
    # =========================================================================
    s.add("StdMath", "math constants", """
        import "std.math"
        println(math.PI > 3.14)
        println(math.E > 2.71)
    """, expected="true\ntrue")
    
    s.add("StdMath", "trig functions", """
        import "std.math"
        println(math.sin(0))
        println(math.cos(0))
    """, expect_contains=["0", "1"])
    
    s.add("StdMath", "math helpers", """
        import "std.math"
        println(math.abs(-5))
        println(math.clamp(15, 0, 10))
    """, expect_contains=["5", "10"])
    
    # =========================================================================
    # 38. STD.COLLECTIONS
    # =========================================================================
    s.add("StdCollections", "list constructor", """
        import "std.collections"
        println(list([1, 2, 3]))
    """, expect_contains=["1", "2", "3"])
    
    # =========================================================================
    # 39. OPTIONAL CHAINING
    # =========================================================================
    s.add("OptionalChaining", "?. on null", """
        let x = null
        println(x?.name)
    """, expected="null")
    
    s.add("OptionalChaining", "?. on value", """
        class Obj {
            func constructor(name) { self.name = name }
        }
        let x = new Obj("hello")
        println(x?.name)
    """, expected="hello")
    
    # =========================================================================
    # 40. CAST (as)
    # =========================================================================
    s.add("Cast", "as type cast", """
        let x = 3.14 as int
        println(x)
    """, expected="3")
    
    # =========================================================================
    # 41. TYPEOF
    # =========================================================================
    s.add("TypeOf", "typeof expression", """
        println(typeof(42))
        println(typeof("hello"))
    """, expect_contains=["int", "str"])
    
    # =========================================================================
    # 42. WHILE LET
    # =========================================================================
    s.add("WhileLet", "while let Some", """
        let items = [Some(1), Some(2), None, Some(4)]
        let i = 0
        while let Some(v) = items[i] {
            println(v)
            i += 1
        }
    """, expected="1\n2")
    
    # =========================================================================
    # 43. PRINT OPTIONS
    # =========================================================================
    s.add("PrintOpts", "print with sep and end", """
        print(1, 2, 3, sep: ", ", end: "!\\n")
    """, expected="1, 2, 3!")
    
    # =========================================================================
    # 44. USING
    # =========================================================================
    # NOTE: using injects fields (not methods) into scope. Works with dicts and instance fields.
    s.add("Using", "using statement", """
        let config = {"host": "localhost", "port": 8080}
        using config {
            println(host)
            println(port)
        }
    """, expected="localhost\n8080")
    
    # =========================================================================
    # 45. LABEL AND GOTO
    # =========================================================================
    # NOTE: goto only works inside blocks at same nesting level. Forward jumps only reliably.
    s.add("LabelGoto", "basic label and goto", """
        {
            goto skip_this
            println("SHOULD NOT SEE")
            label skip_this:
            println("skipped")
        }
    """, expected="skipped")
    
    # =========================================================================
    # 46. ISOLATE
    # =========================================================================
    s.add("Isolate", "isolate block parses", """
        isolate {
            let x = 42
        }
        println("ok")
    """, expected="ok")
    
    # =========================================================================
    # 47. UNSAFE
    # =========================================================================
    s.add("Unsafe", "unsafe block", """
        unsafe {
            let x = 42
            println(x)
        }
    """, expected="42")
    
    # =========================================================================
    # 48. BITFIELD
    # =========================================================================
    s.add("Bitfield", "bitfield struct parses", """
        bitfield struct Flags {
            read: 1
            write: 1
            exec: 1
        }
        println("parsed ok")
    """, expected="parsed ok")
    
    # =========================================================================
    # 49. ENABLE / EMBEDDED LANGS
    # =========================================================================
    s.add("Embedded", "enable block parses", """
        enable { python }
        println("ok")
    """, expected="ok")
    
    # =========================================================================
    # 50. EVAL
    # =========================================================================
    s.add("Eval", "eval expression", """
        let result = eval("1 + 2")
        println(result)
    """, expected="3")
    
    # =========================================================================
    # 51. VARS BUILTIN
    # =========================================================================
    s.add("Vars", "vars() introspection", """
        class Obj {
            func constructor() {
                self.x = 1
                self.y = 2
            }
        }
        let o = new Obj()
        let v = vars(o)
        println(v)
    """, expect_contains=["x", "y", "1", "2"])
    
    # =========================================================================
    # 52. COMPILATION MODES
    # =========================================================================
    s.add("Compile", "compile to bytecode", """
        println("hello")
    """, compile_mode=True, expect_parse_ok=True)
    
    # =========================================================================
    # 53. ACTOR/AGENT (smoke test)
    # =========================================================================
    s.add("Actors", "actor definition parses", """
        actor Counter {
            let count = 0
            func increment() {
                self.count += 1
            }
        }
        println("parsed ok")
    """, expected="parsed ok")
    
    s.add("Agents", "agent definition parses", """
        agent Greeter "greet users" {
            func run() {
                return "hello"
            }
        }
        println("parsed ok")
    """, expected="parsed ok")
    
    # =========================================================================
    # 54. CSTRUCT
    # =========================================================================
    s.add("CStruct", "cstruct definition", """
        cstruct Header { magic: int, version: int }
        println("parsed ok")
    """, expected="parsed ok")
    
    return s

# =============================================================================
if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="V2 Documentation Compliance Test Suite")
    parser.add_argument("--section", help="Filter to sections matching pattern")
    parser.add_argument("--verbose", "-v", action="store_true")
    parser.add_argument("--compile", action="store_true")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    
    suite = build_suite()
    results = suite.run_all(
        filter_pattern=args.section, verbose=args.verbose,
        also_compile=args.compile, keep=args.keep
    )
    
    failed = sum(1 for r in results if not r.passed)
    sys.exit(1 if failed > 0 else 0)
