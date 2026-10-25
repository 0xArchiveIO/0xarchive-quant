"""Deterministic math: oxarchive_backtest.math behaves like CPython's math (same exceptions,
same special values) while giving fdlibm's bits, `**` is made deterministic by load_strategy,
and check_determinism finds what is left.

Run: python -m pytest crates/py/tests  (after installing the wheel)
"""

import math
import pathlib
import struct
import warnings

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import _determinism
from oxarchive_backtest._determinism import _ORIGINAL

import log_momentum

# Pinned: every build target prints the same.
MATH_CHECK_HASH = "0e4c187b4645d2e1"
# 23 functions x 50,000 of the reference inputs, each pinned against x64 V8.
SPECIAL = [0.0, -0.0, 1.0, -1.0, 0.5, -0.5, 2.0, 3, -3, 1e-310, 1e308, -1e308, 710.0, -800.0,
           1000.0, math.inf, -math.inf, math.nan, True, 10**400, -(10**400)]  # fmt: skip


def outcome(f, *args):
    try:
        r = f(*args)
    except Exception as e:  # noqa: BLE001 - comparing which exception
        return type(e).__name__, str(e)
    return "ok", r


def close(a, b):
    if isinstance(a, float) and isinstance(b, float):
        if math.isnan(a) or math.isnan(b):
            return math.isnan(a) and math.isnan(b)
        if a == 0 or b == 0 or math.isinf(a) or math.isinf(b):
            return struct.pack("<d", a) == struct.pack("<d", b)
        return abs(a - b) <= 4e-16 * max(abs(a), abs(b))
    return a == b


def test_stdlib_math_is_replaced():
    assert math.exp is oxb.math.exp and math.log is oxb.math.log and math.pow is oxb.math.pow
    # V8's fdlibm value; Apple's libm gives 1.0986122886681098.
    assert math.log(3) == 1.0986122886681096
    assert math.exp(1) == math.e


def test_math_check_hash_matches_native_and_wasm():
    assert oxb.math.check_hash() == MATH_CHECK_HASH


@pytest.mark.parametrize("name", [n for n in _ORIGINAL if n not in ("pow", "atan2", "hypot", "dist")])
def test_unary_raises_and_special_values_like_cpython(name):
    ours, theirs = getattr(oxb.math, name), _ORIGINAL[name]
    for x in SPECIAL:
        a, b = outcome(ours, x), outcome(theirs, x)
        assert a[0] == b[0], f"{name}({x!r}): {a} vs CPython {b}"
        if a[0] == "ok":
            assert close(a[1], b[1]), f"{name}({x!r}): {a[1]!r} vs CPython {b[1]!r}"
        elif a[0] == "ValueError" and name not in ("exp2",):
            assert a[1] == b[1], f"{name}({x!r}) message"


@pytest.mark.parametrize("name", ["pow", "atan2", "log"])
def test_binary_raises_and_special_values_like_cpython(name):
    ours, theirs = getattr(oxb.math, name), _ORIGINAL[name]
    for x in SPECIAL:
        for y in SPECIAL:
            if name == "log" and isinstance(y, int) and y > 10**300:
                continue
            a, b = outcome(ours, x, y), outcome(theirs, x, y)
            assert a[0] == b[0], f"{name}({x!r}, {y!r}): {a} vs CPython {b}"
            if a[0] == "ok":
                assert close(a[1], b[1]), f"{name}({x!r}, {y!r}): {a[1]!r} vs {b[1]!r}"


def test_hypot_and_dist():
    assert math.hypot() == 0.0 and math.hypot(3, 4, 12) == 13.0
    assert math.hypot(math.inf, math.nan) == math.inf
    assert math.dist([1, 2], [4, 6]) == 5.0
    with pytest.raises(ValueError, match="same number of dimensions"):
        math.dist([1, 2], [1])


def test_deterministic_pow_matches_python_semantics():
    p = _determinism.deterministic_pow
    values = [0.0, -0.0, 1.0, -1.0, 2.0, -2.0, 0.5, 3, -3, 0, 7, 1e300, -1e300, 1.5, -1.5,
              math.inf, -math.inf, math.nan, True, 1 / 3]  # fmt: skip
    for x in values:
        for y in values:
            a, b = outcome(p, x, y), outcome(lambda u, v: u**v, x, y)
            assert a[0] == b[0], f"{x!r} ** {y!r}: {a} vs Python {b}"
            if a[0] == "ok":
                assert type(a[1]) is type(b[1]), f"{x!r} ** {y!r}: type"
                if isinstance(a[1], complex):
                    continue
                assert close(a[1], b[1]), f"{x!r} ** {y!r}: {a[1]!r} vs {b[1]!r}"
    assert p(2, 100) == 2**100 and type(p(2, 100)) is int
    assert p(3, 4, 5) == pow(3, 4, 5)
    assert p(2.0, 0.5) == oxb.math.pow(2.0, 0.5)


def test_load_strategy_rewrites_pow(tmp_path):
    src = (
        "def f(x):\n"
        "    y = x ** 2\n"
        "    y **= 0.5\n"
        "    return y, pow(x, 3), 2 ** 10\n"
    )
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        mod = oxb.load_strategy(source=src, name="pow_rewrite_test")
    x = 1.0 / 3.0
    y, cube, ints = mod.f(x)
    assert y == oxb.math.pow(oxb.math.pow(x, 2.0), 0.5)
    assert cube == oxb.math.pow(x, 3.0)
    assert ints == 1024 and type(ints) is int
    assert oxb.check_determinism(mod) == []


def test_load_strategy_checks_the_classes_in_the_module():
    src = "import math\n\nclass S:\n    def on_bar(self, ctx, bar):\n        return math.erf(1.0)\n"
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        mod = oxb.load_strategy(source=src, name="class_check_test")
    assert [(w.lineno, "math.erf" in str(w.message)) for w in caught] == [(5, True)]
    assert len(oxb.check_determinism(mod)) == 1


def test_check_determinism_finds_what_is_left():
    np = pytest.importorskip("numpy")
    src = (
        "import math\n"
        "import numpy as np\n"
        "from numpy import tanh\n"
        "class S:\n"
        "    def on_bar(self, ctx, bar):\n"
        "        a = np.exp(np.arange(3.0))\n"  # line 6
        "        b = tanh(a)\n"  # line 7
        "        c = math.erf(0.5)\n"  # line 8
        "        d = helper(a)\n"
        "def helper(a):\n"
        "    return np.dot(a, a)\n"  # line 11
    )
    ns = {"__name__": "check_test"}
    exec(compile(src, "<check_test>", "exec"), ns)
    found = oxb.check_determinism(ns["S"])
    lines = sorted(int(i.split(":")[1]) for i in found)
    assert lines == [6, 7, 8, 11], found
    assert any("oxarchive_backtest.math.exp" in i for i in found)
    assert any("BLAS" in i for i in found)

    def uses_pow(x):
        return x**2 + pow(x, 2)

    assert len(oxb.check_determinism(uses_pow)) == 2

    ns = {"__name__": "early_import", "exp": _ORIGINAL["exp"]}
    exec("def g(x):\n    return exp(x)\n", ns)
    (issue,) = oxb.check_determinism(ns["g"])
    assert "imported before oxarchive_backtest" in issue
    assert np is not None


def test_backtest_warns_once(tmp_path):
    from refdata import T0, end, write

    root = write(tmp_path, "REF", 50)

    def squares(h):
        c = h.close.last()["REF"]
        return None if c is None else c**2

    s = oxb.Strategy(universe=["REF"], clock="1m", bars="1m", step=lambda ctx, h: squares(h))
    with pytest.warns(oxb.DeterminismWarning, match=r"\*\* on floats"):
        oxb.backtest(s, T0, end(50), root)
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        oxb.backtest(s, T0, end(50), root)


def test_a_strategy_built_in_a_loaded_file_is_checked():
    src = (
        "import math\nimport oxarchive_backtest as oxb\n"
        "strategy = oxb.Strategy(universe=['X'], weights=lambda s, h: math.erf(1.0))\n"
    )
    with pytest.warns(oxb.DeterminismWarning, match="math.erf"):
        mod = oxb.load_strategy(source=src, name="lambda_check_test")
    assert len(oxb.check_determinism(mod.strategy)) == 1


def test_arrays():
    np = pytest.importorskip("numpy")
    xs = np.array([[-3.0, 0.0, 0.5], [2.0, 1e300, np.nan]])
    for name in ("exp", "log", "sin", "tanh", "cbrt", "atanh", "log1p"):
        got = getattr(oxb.math, name)(xs)
        assert got.shape == xs.shape and got.dtype == np.float64
        for g, x in zip(got.ravel(), xs.ravel(), strict=True):
            kind, want = outcome(getattr(oxb.math, name), float(x))
            if kind == "ok":  # where the scalar raises, the array gives NaN or an infinity
                assert close(float(g), want), (name, x)
    assert np.isnan(oxb.math.log(np.array([-1.0])))[0]
    assert oxb.math.log(np.array([0.0]))[0] == -np.inf
    p = oxb.math.pow(np.array([[2.0], [3.0]]), np.array([0.5, 2.0, 3.0]))
    assert p.shape == (2, 3) and p[1, 1] == 9.0 and p[0, 0] == oxb.math.pow(2.0, 0.5)
    assert oxb.math.hypot(np.array([3.0, 5.0]), 4.0).tolist() == [5.0, oxb.math.hypot(5.0, 4.0)]
    assert _determinism.deterministic_pow(np.array([2.0, 4.0]), 0.5).tolist() == [oxb.math.pow(2.0, 0.5), 2.0]
    assert _determinism.deterministic_pow(np.array([2, 3]), 2).tolist() == [4, 9]


def test_log_momentum_runs_and_control_differs_from_ours(tmp_path):
    from refdata import write

    root = write(tmp_path, "REF", 5_000)
    ours = log_momentum.run(oxb, root)
    assert ours[0] > 10, ours
    assert ours == log_momentum.run(oxb, root), "stable"
    print(f"log_momentum ours={ours} platform={log_momentum.run(oxb, root, platform_math=True)}")
    assert pathlib.Path(log_momentum.__file__).exists()
