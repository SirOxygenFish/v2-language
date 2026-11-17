"""Pool workers that print at once (tests/engines/py/fork_lines.v2)."""


def chatty(i):
    for n in range(50):
        print("child", i, n, "x" * 40)
    return i
