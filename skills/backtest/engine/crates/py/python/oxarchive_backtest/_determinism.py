"""Keeping a strategy's floating-point results the same on every machine.

Three things in plain Python give different last bits on different platforms, because CPython
hands them to the operating system's C library (measured: macOS CPython
against WebAssembly, a million inputs each):

1. math.exp, log, sin, tanh and the rest. `install_math` replaces them in the standard `math`
   module with oxarchive_backtest.math. Importing the package calls it.
2. Float `**` and pow(), even `x ** 2`: about 0.1% of results differ, and neither platform always
   equals `x * x`. `load_strategy` rewrites `**` and `pow` in the strategy's source into
   `deterministic_pow`. Code that is not loaded that way gets a warning instead.
3. NumPy's own transcendental functions and its BLAS calls (dot, matmul, linalg). They are not
   replaced; `check_determinism` finds them and backtest warns. oxarchive_backtest.math's
   functions take arrays as the deterministic alternative.

Plain + - * / and math.sqrt are exact everywhere and need nothing.
"""

import ast
import builtins
import copy
import dis
import linecache
import math as _std_math
import sys
import types
import warnings
import weakref

from . import math as _det

# The math functions oxarchive_backtest.math replaces.
PATCHED = (
    "acos", "acosh", "asin", "asinh", "atan", "atan2", "atanh", "cbrt", "cos", "cosh", "dist",
    "exp", "exp2", "expm1", "hypot", "log", "log10", "log1p", "log2", "pow", "sin", "sinh",
    "tan", "tanh",
)  # fmt: skip
# The platform's own versions, captured before install_math replaces them.
_ORIGINAL = {name: getattr(_std_math, name) for name in PATCHED if hasattr(_std_math, name)}
# math functions built on the platform's libm that have no deterministic version yet.
UNCOVERED = ("erf", "erfc", "gamma", "lgamma")

# NumPy functions whose results depend on the machine: its own transcendental loops and BLAS.
_NUMPY_TRANSCENDENTAL = {
    "exp", "exp2", "expm1", "log", "log1p", "log2", "log10", "logaddexp", "logaddexp2",
    "power", "float_power", "sin", "cos", "tan", "arcsin", "arccos", "arctan", "arctan2",
    "sinh", "cosh", "tanh", "arcsinh", "arccosh", "arctanh", "cbrt", "hypot", "sinc",
}  # fmt: skip
_NUMPY_BLAS = {"dot", "vdot", "matmul", "inner", "tensordot", "einsum", "linalg"}

_POW = "__oxs_pow__"


class DeterminismWarning(UserWarning):
    """The strategy uses something whose result can differ between machines."""


def install_math():
    """Puts oxarchive_backtest.math into the standard `math` module. Also fixes `random` and
    `statistics` if already imported, since they bind math functions by name at import."""
    for name in _ORIGINAL:
        setattr(_std_math, name, getattr(_det, name))
    for modname in ("random", "statistics"):
        mod = sys.modules.get(modname)
        if mod is None:
            continue
        for key, value in list(vars(mod).items()):
            name = _original_name(value)
            if name is not None:
                setattr(mod, key, getattr(_det, name))


def _original_name(value):
    """The math name if `value` is one of the platform's original math functions."""
    for name, f in _ORIGINAL.items():
        if value is f:
            return name
    return None


def _float_array_pow(base, exp):
    np = sys.modules.get("numpy")
    if np is None or not (isinstance(base, np.ndarray) or isinstance(exp, np.ndarray)):
        return False
    try:
        return np.result_type(base, exp) == np.float64
    except TypeError:
        return False


def deterministic_pow(base, exp, mod=None):
    """`**` and pow() with CPython's semantics, but oxarchive_backtest.math's pow for floats.

    Ints stay exact (int ** non-negative int is builtin pow). A float or a negative int
    exponent goes through float_pow, which raises where CPython raises. A negative base to a
    fractional power is left to Python, which returns a complex number. float64 NumPy arrays
    go through oxarchive_backtest.math.pow; anything else is builtin pow.
    """
    if mod is None:
        if isinstance(base, (int, float)) and isinstance(exp, (int, float)):
            if isinstance(base, float) or isinstance(exp, float) or exp < 0:
                r = _det.float_pow(float(base), float(exp))
                if r is not None:
                    return r
        elif _float_array_pow(base, exp):
            return _det.pow(base, exp)
        return builtins.pow(base, exp)
    return builtins.pow(base, exp, mod)


class _RewritePow(ast.NodeTransformer):
    """a ** b -> __oxs_pow__(a, b); a **= b -> a = __oxs_pow__(a, b). For an attribute or
    subscript target, its object and index are evaluated twice."""

    def visit_BinOp(self, node):
        self.generic_visit(node)
        if isinstance(node.op, ast.Pow):
            call = ast.Call(ast.Name(_POW, ast.Load()), [node.left, node.right], [])
            return ast.copy_location(call, node)
        return node

    def visit_AugAssign(self, node):
        self.generic_visit(node)
        if not isinstance(node.op, ast.Pow):
            return node
        current = copy.deepcopy(node.target)
        current.ctx = ast.Load()
        call = ast.Call(ast.Name(_POW, ast.Load()), [current, node.value], [])
        return ast.copy_location(ast.Assign([node.target], call), node)


def load_strategy(path=None, *, source=None, name="strategy"):
    """Loads a strategy module from a file or a source string the way every 0xArchive surface
    does: deterministic math installed, `**` and pow()
    rewritten to deterministic_pow, then warnings for anything still machine-dependent.
    Returns the module; it is also registered as sys.modules[name]."""
    if (path is None) == (source is None):
        raise TypeError("load_strategy takes a path or source=, not both")
    install_math()
    if path is not None:
        filename = str(path)
        with open(filename, encoding="utf-8") as f:
            source = f.read()
    else:
        filename = f"<{name}>"
        # Lets tracebacks and inspect show the source.
        linecache.cache[filename] = (len(source), None, source.splitlines(True), filename)
    tree = ast.fix_missing_locations(_RewritePow().visit(ast.parse(source, filename)))
    code = compile(tree, filename, "exec")
    module = types.ModuleType(name)
    module.__file__ = filename
    module.__dict__[_POW] = deterministic_pow
    module.__dict__["pow"] = deterministic_pow
    sys.modules[name] = module
    exec(code, module.__dict__)
    # The module's own top-level code too: steps are often lambdas passed straight to
    # oxb.Strategy(...).
    _warn(_issues(module, extra=[types.FunctionType(code, module.__dict__)]))
    return module


def _code_objects(code):
    yield code
    for const in code.co_consts:
        if isinstance(const, types.CodeType):
            yield from _code_objects(const)


def _functions(obj):
    """The Python functions directly inside a class or module."""
    if isinstance(obj, types.FunctionType):
        return [obj]
    found = []
    for value in vars(obj).values():
        if isinstance(value, (staticmethod, classmethod)):
            value = value.__func__
        elif isinstance(value, property):
            found.extend(f for f in (value.fget, value.fset, value.fdel) if f is not None)
            continue
        if isinstance(value, types.FunctionType):
            found.append(value)
    return found


def _numpy_name(value):
    """'numpy.exp' if value is one of NumPy's machine-dependent functions, else None."""
    name = getattr(value, "__name__", None)
    module = getattr(value, "__module__", None) or ""
    is_ufunc = type(value).__name__ == "ufunc" and type(value).__module__ == "numpy"
    if name in _NUMPY_TRANSCENDENTAL | _NUMPY_BLAS and (is_ufunc or module.startswith("numpy")):
        return f"numpy.{name}"
    return None


def _message(kind, what):
    if kind == "pow":
        return (
            f"{what} on floats uses this machine's pow(), whose last bit differs between "
            "machines (even x ** 2). Load the strategy with oxarchive_backtest.load_strategy(), "
            "which makes ** and pow() deterministic, or use math.pow()."
        )
    if kind == "numpy":
        if what.split(".")[-1] in _NUMPY_BLAS:
            return f"{what} uses this machine's BLAS; its result can differ between machines."
        return (
            f"{what} uses NumPy's own math, whose last bit differs between machines. Use "
            f"oxarchive_backtest.math.{_numpy_to_math(what)}, which takes arrays."
        )
    if kind == "libm":
        return (
            f"{what} is the platform's version, imported before oxarchive_backtest. Import "
            "oxarchive_backtest first, or call it as math.<name>."
        )
    return f"{what} has no deterministic version; its last bit can differ between machines."


def _numpy_to_math(what):
    name = what.split(".")[-1]
    renames = {"power": "pow", "float_power": "pow", "arcsin": "asin", "arccos": "acos",
               "arctan": "atan", "arctan2": "atan2", "arcsinh": "asinh", "arccosh": "acosh",
               "arctanh": "atanh"}  # fmt: skip
    return renames.get(name, name)


def check_determinism(obj):
    """Lists what in a strategy (a Strategy, an instance, class, function or module) can give different
    results on different machines, as "file:line: message" strings. Follows the functions
    and classes of the same module that it calls. Reads bytecode, so it works without source.
    """
    return [f"{f}:{line}: {msg}" for f, line, msg in _issues(obj)]


def _issues(obj, extra=()):
    todo = list(extra)
    if hasattr(obj, "_functions") and not isinstance(obj, type):
        todo.extend(obj._functions())  # an oxb.Strategy: the functions it calls
        obj = None
    elif not isinstance(obj, (type, types.ModuleType, types.FunctionType)):
        obj = type(obj)
    issues = []
    seen = set()
    if obj is not None:
        todo.append(obj)
    if isinstance(obj, type):
        todo.extend(c for c in obj.__mro__[1:] if c.__module__ not in ("builtins",))
    uncovered = {id(getattr(_std_math, n)): n for n in UNCOVERED if hasattr(_std_math, n)}
    while todo:
        item = todo.pop()
        if id(item) in seen:
            continue
        seen.add(id(item))
        if isinstance(item, types.ModuleType):
            # The module's own classes, such as the strategy class itself.
            todo.extend(
                v for v in vars(item).values()
                if isinstance(v, type) and v.__module__ == item.__name__
            )  # fmt: skip
        for fn in _functions(item):
            g = fn.__globals__
            home = fn.__module__
            for code in _code_objects(fn.__code__):
                chain = None  # "numpy" or "math" after loading that module
                for ins in dis.get_instructions(code):
                    line = (ins.positions.lineno if ins.positions else None) or 0
                    where = (code.co_filename, line)
                    op = ins.opname
                    if op == "BINARY_OP" and ins.argrepr in ("**", "**="):
                        issues.append((where, _message("pow", "**")))
                    elif op in ("BINARY_POWER", "INPLACE_POWER"):
                        issues.append((where, _message("pow", "**")))
                    elif op in ("LOAD_GLOBAL", "LOAD_NAME"):
                        name = ins.argval
                        chain = None
                        if name in g:
                            value = g[name]
                        elif name == "pow":
                            issues.append((where, _message("pow", "pow()")))
                            continue
                        else:
                            continue
                        if value is _std_math:
                            chain = "math"
                        elif getattr(value, "__name__", None) == "numpy" and isinstance(value, types.ModuleType):
                            chain = "numpy"
                        elif (orig := _original_name(value)) is not None:
                            issues.append((where, _message("libm", f"{name} (math.{orig})")))
                        elif id(value) in uncovered:
                            issues.append((where, _message("uncovered", f"math.{uncovered[id(value)]}")))
                        elif (np_name := _numpy_name(value)) is not None:
                            issues.append((where, _message("numpy", np_name)))
                        elif getattr(value, "__module__", None) == home and isinstance(value, (type, types.FunctionType)):
                            todo.append(value)
                    elif op in ("LOAD_ATTR", "LOAD_METHOD") and chain is not None:
                        attr = ins.argval
                        if chain == "numpy" and attr in _NUMPY_TRANSCENDENTAL | _NUMPY_BLAS:
                            issues.append((where, _message("numpy", f"numpy.{attr}")))
                        elif chain == "math" and attr in UNCOVERED:
                            issues.append((where, _message("uncovered", f"math.{attr}")))
                        chain = None
                    elif op not in ("CACHE", "EXTENDED_ARG", "PUSH_NULL"):
                        chain = None
    return sorted({(f, line, msg) for (f, line), msg in issues})


_registry = {}
# Strategies already checked; weak, so a new strategy at a freed one's address is checked.
_checked = weakref.WeakSet()
_checked_types = set()


def _warn(issues):
    for filename, line, msg in issues:
        warnings.warn_explicit(msg, DeterminismWarning, filename, line, registry=_registry)


def warn_determinism(strategy):
    """Warns once per issue for what check_determinism finds in this strategy."""
    if hasattr(strategy, "_functions"):
        if strategy in _checked:
            return
        _checked.add(strategy)
    else:
        if type(strategy) in _checked_types:
            return
        _checked_types.add(type(strategy))
    _warn(_issues(strategy))
