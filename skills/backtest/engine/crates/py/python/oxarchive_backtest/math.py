"""Deterministic versions of the `math` functions that depend on the platform's C library.

CPython's math.exp, log, sin, pow and the rest call the operating system's libm, and macOS,
Linux and WebAssembly give different last bits for the same input. These are one
implementation (fdlibm, ported in Rust as crates/math) that gives the same bits everywhere, and
the same bits as 0xArchive's other script engines.

Importing oxarchive_backtest puts them into the standard `math` module, so strategies just use
`math`. Each also takes a NumPy array and returns a float64 array, the deterministic stand-in
for numpy.exp and friends. Numbers raise like CPython 3.14's math; arrays give NaN and
infinities like NumPy.

    exp exp2 expm1 log log1p log2 log10 pow
    sin cos tan asin acos atan atan2
    sinh cosh tanh asinh acosh atanh
    cbrt hypot dist
"""

from ._native import math as _m

acos = _m.acos
acosh = _m.acosh
asin = _m.asin
asinh = _m.asinh
atan = _m.atan
atan2 = _m.atan2
atanh = _m.atanh
cbrt = _m.cbrt
cos = _m.cos
cosh = _m.cosh
dist = _m.dist
exp = _m.exp
exp2 = _m.exp2
expm1 = _m.expm1
hypot = _m.hypot
log = _m.log
log10 = _m.log10
log1p = _m.log1p
log2 = _m.log2
pow = _m.pow
sin = _m.sin
sinh = _m.sinh
tan = _m.tan
tanh = _m.tanh

# CPython's float ** float on two floats (None where Python would give a complex number);
# load_strategy routes ** through it.
float_pow = _m.float_pow
# The cross-target check's hash: equal on two machines means every function agrees there.
check_hash = _m.check_hash

__all__ = [
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atan2",
    "atanh",
    "cbrt",
    "cos",
    "cosh",
    "dist",
    "exp",
    "exp2",
    "expm1",
    "hypot",
    "log",
    "log10",
    "log1p",
    "log2",
    "pow",
    "sin",
    "sinh",
    "tan",
    "tanh",
]
