"""The curated package set: the Python packages a strategy may import, at the
versions the browser (Pyodide), the wheel and the runner all use.

The browser can only load what Pyodide ships, at Pyodide's version. The wheel runs ordinary
CPython, where anything installs, so a strategy that imports something else, or runs on a
different numpy, could work locally and then fail or give other trades in the browser.
`check_imports` and `check_installed` catch that before a version is saved (`0xbt check`).

    pip install '0xarchive-backtest[curated]'     # exactly these versions

The versions are Pyodide 314.0.7's. scripts/check-pyodide.sh fails if they drift from the
Pyodide it tests against.
"""

import ast
import importlib.metadata
import sys

PYODIDE_VERSION = "314.0.7"

# Distribution name -> version. Strategies import the first three; the rest are their
# dependencies, pinned so a local install resolves to the same set.
PACKAGES = {
    "numpy": "2.4.6",
    "pandas": "3.0.2",
    "scipy": "1.18.0",
    "python-dateutil": "2.9.0.post0",
    "pytz": "2026.1.post1",
    "six": "1.17.0",
}

# Top-level import name -> distribution name, for the packages above and this one.
IMPORTABLE = {
    "numpy": "numpy",
    "pandas": "pandas",
    "scipy": "scipy",
    "dateutil": "python-dateutil",
    "pytz": "pytz",
    "six": "six",
    "oxarchive_backtest": None,
}

# Standard-library modules a strategy may not import, and why. Everything else in the
# standard library is allowed.
_NO_THREADS = "no threads or processes in the browser"
_NO_NETWORK = "no network access during a backtest; data comes from the dataset"
_NOT_IN_PYODIDE = "not in Pyodide"
BLOCKED = {
    "_thread": _NO_THREADS,
    "concurrent": _NO_THREADS,
    "multiprocessing": _NO_THREADS,
    "subprocess": _NO_THREADS,
    "threading": _NO_THREADS,
    "ftplib": _NO_NETWORK,
    "http": _NO_NETWORK,
    "imaplib": _NO_NETWORK,
    "poplib": _NO_NETWORK,
    "smtplib": _NO_NETWORK,
    "socket": _NO_NETWORK,
    "socketserver": _NO_NETWORK,
    "ssl": _NO_NETWORK,
    "urllib.request": _NO_NETWORK,
    "webbrowser": _NO_NETWORK,
    "xmlrpc": _NO_NETWORK,
    "curses": _NOT_IN_PYODIDE,
    "dbm": _NOT_IN_PYODIDE,
    "ensurepip": _NOT_IN_PYODIDE,
    "idlelib": _NOT_IN_PYODIDE,
    "readline": _NOT_IN_PYODIDE,
    "tkinter": _NOT_IN_PYODIDE,
    "turtle": _NOT_IN_PYODIDE,
    "turtledemo": _NOT_IN_PYODIDE,
    "venv": _NOT_IN_PYODIDE,
}


def _blocked_reason(module):
    parts = module.split(".")
    for i in range(len(parts), 0, -1):
        reason = BLOCKED.get(".".join(parts[:i]))
        if reason is not None:
            return reason
    return None


def _module_problem(module):
    top = module.split(".")[0]
    if top == "__future__":
        return None
    reason = _blocked_reason(module)
    if reason is not None:
        return f"import {module}: {reason}"
    if top in IMPORTABLE or top in sys.stdlib_module_names:
        return None
    return (
        f"import {module}: not in the curated package set (numpy, pandas, scipy and the "
        "standard library), so it would fail in the browser and on the runner"
    )


def imported_modules(source, filename="<strategy>"):
    """(line, module) for every absolute import in the source, including ones inside
    functions. A relative import comes back as (line, None)."""
    found = []
    for node in ast.walk(ast.parse(source, filename)):
        if isinstance(node, ast.Import):
            found.extend((node.lineno, alias.name) for alias in node.names)
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                found.append((node.lineno, None))
            else:
                found.append((node.lineno, node.module))
                # `from urllib import request` imports urllib.request.
                found.extend((node.lineno, f"{node.module}.{a.name}") for a in node.names
                             if _blocked_reason(f"{node.module}.{a.name}") is not None)  # fmt: skip
    return sorted(set(found), key=lambda x: (x[0], x[1] or ""))


def check_imports(source, filename="<strategy>"):
    """What the strategy imports that the browser or the runner can't, as
    "file:line: message" strings. Empty when every import is in the curated set."""
    problems = []
    for line, module in imported_modules(source, filename):
        if module is None:
            msg = "relative import: a strategy is one file, so it has nothing to import from"
        else:
            msg = _module_problem(module)
        if msg is not None:
            problems.append(f"{filename}:{line}: {msg}")
    return problems


def check_installed(source=None, filename="<strategy>"):
    """Curated packages installed here at a different version than the browser's, as
    messages; a different version can give different trades. With `source`, nothing unless
    that strategy imports a curated package (then all of them, since they depend on each
    other)."""
    if source is not None:
        tops = {m.split(".")[0] for _, m in imported_modules(source, filename) if m}
        if not any(IMPORTABLE.get(t) for t in tops):
            return []
    problems = []
    for dist in sorted(PACKAGES):
        try:
            have = importlib.metadata.version(dist)
        except importlib.metadata.PackageNotFoundError:
            continue
        want = PACKAGES[dist]
        if have != want:
            problems.append(
                f"{dist} {have} is installed; the browser and the runner use {want}, and "
                f"results can differ. pip install '0xarchive-backtest[curated]'"
            )
    return problems
