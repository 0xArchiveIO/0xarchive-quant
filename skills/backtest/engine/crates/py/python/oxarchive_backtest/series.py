"""Series: one value per market, None where unknown, in universe order. What signals and
weights pass between a strategy's steps.

    s = h.close.ret("7d") / h.close.vol("30d")
    s.rank()["BTC"]       # 0.8
    s.top(5)              # the five largest, largest first

Arithmetic and comparisons work element by element with another Series (matched by
market) or a number; None stays None, and so does a division by zero. Sums are plain
loops, so results match on every machine (Python's built-in sum() rounds differently).
"""

import math as _std

from . import math as _math


class Series:
    __slots__ = ("_d",)
    __hash__ = None

    def __init__(self, data=None):
        if data is None:
            self._d = {}
        elif isinstance(data, Series):
            self._d = dict(data._d)
        else:
            self._d = dict(data)

    @classmethod
    def _of(cls, keys, values):
        s = cls.__new__(cls)
        s._d = dict(zip(keys, values))
        return s

    # Reading like a dict.

    def __getitem__(self, market):
        return self._d[market]

    def get(self, market, default=None):
        return self._d.get(market, default)

    def __contains__(self, market):
        return market in self._d

    def __iter__(self):
        return iter(self._d)

    def __len__(self):
        return len(self._d)

    def keys(self):
        return self._d.keys()

    def values(self):
        return self._d.values()

    def items(self):
        return self._d.items()

    def to_dict(self):
        return dict(self._d)

    def to_pandas(self):
        import pandas

        return pandas.Series(self._d, dtype="float64")

    def __repr__(self):
        inner = ", ".join(f"{k}: {'None' if v is None else f'{v:.6g}'}" for k, v in self._d.items())
        return f"Series({{{inner}}})"

    # Element by element.

    def _map(self, f):
        return Series._of(self._d.keys(), [None if v is None else f(v) for v in self._d.values()])

    def _zip(self, other, f):
        if isinstance(other, Series):
            o = other._d
            out = []
            for k, a in self._d.items():
                b = o.get(k)
                out.append(None if a is None or b is None else f(a, b))
            return Series._of(self._d.keys(), out)
        if other is None:
            return Series._of(self._d.keys(), [None] * len(self._d))
        return self._map(lambda a: f(a, other))

    def __add__(self, o):
        return self._zip(o, lambda a, b: a + b)

    def __radd__(self, o):
        return self._zip(o, lambda a, b: b + a)

    def __sub__(self, o):
        return self._zip(o, lambda a, b: a - b)

    def __rsub__(self, o):
        return self._zip(o, lambda a, b: b - a)

    def __mul__(self, o):
        return self._zip(o, lambda a, b: a * b)

    def __rmul__(self, o):
        return self._zip(o, lambda a, b: b * a)

    def __truediv__(self, o):
        return self._zip(o, lambda a, b: a / b if b != 0 else None)

    def __rtruediv__(self, o):
        return self._zip(o, lambda a, b: b / a if a != 0 else None)

    def __pow__(self, o):
        return self._zip(o, _math.float_pow)

    def __neg__(self):
        return self._map(lambda a: -a)

    def __abs__(self):
        return self._map(abs)

    def __lt__(self, o):
        return self._zip(o, lambda a, b: a < b)

    def __le__(self, o):
        return self._zip(o, lambda a, b: a <= b)

    def __gt__(self, o):
        return self._zip(o, lambda a, b: a > b)

    def __ge__(self, o):
        return self._zip(o, lambda a, b: a >= b)

    def __eq__(self, o):
        return self._zip(o, lambda a, b: a == b)

    def __ne__(self, o):
        return self._zip(o, lambda a, b: a != b)

    def where(self, mask):
        """The values where `mask` (a Series of bools) is true, None elsewhere."""
        return self._zip(mask, lambda a, m: a if m else None)

    def fillna(self, value):
        return Series._of(self._d.keys(), [value if v is None else v for v in self._d.values()])

    def clip(self, lo=None, hi=None):
        def f(v):
            if lo is not None and v < lo:
                return lo
            if hi is not None and v > hi:
                return hi
            return v

        return self._map(f)

    def dropna(self):
        return Series((k, v) for k, v in self._d.items() if v is not None)

    def log(self):
        """Natural log, deterministic (oxarchive_backtest.math); None at or below 0."""
        return self._map(lambda v: _math.log(v) if v > 0 else None)

    def exp(self):
        return self._map(_math.exp)

    def sqrt(self):
        return self._map(lambda v: _std.sqrt(v) if v >= 0 else None)

    def sign(self):
        return self._map(lambda v: 1.0 if v > 0 else -1.0 if v < 0 else 0.0)

    # Across markets.

    def count(self):
        return sum(1 for v in self._d.values() if v is not None)

    def sum(self):
        s = 0.0
        for v in self._d.values():
            if v is not None:
                s += v
        return s

    def mean(self):
        n = self.count()
        return self.sum() / n if n else None

    def std(self):
        """Sample standard deviation; None for fewer than two values."""
        xs = [v for v in self._d.values() if v is not None]
        if len(xs) < 2:
            return None
        m = 0.0
        for x in xs:
            m += x
        m /= len(xs)
        v = 0.0
        for x in xs:
            v += (x - m) * (x - m)
        return _std.sqrt(v / (len(xs) - 1))

    def rank(self):
        """Percentile rank among the known values, ties averaged, as pandas'
        rank(pct=True): the largest is 1.0."""
        xs = [v for v in self._d.values() if v is not None]
        n = len(xs)

        def r(x):
            below = equal = 0
            for v in xs:
                if v < x:
                    below += 1
                elif v == x:
                    equal += 1
            return (below + (equal + 1) / 2) / n

        return self._map(r)

    def demean(self, by=None):
        """Less the mean of the known values: across all markets, or within each group of
        `by` ({market: group}, such as a sector; a market with no group is left None)."""
        if by is None:
            m = self.mean()
            return self._map(lambda v: None if m is None else v - m)
        groups = {}
        for k, v in self._d.items():
            g = by.get(k)
            if v is not None and g is not None:
                groups.setdefault(g, []).append(v)
        means = {}
        for g, xs in groups.items():
            t = 0.0
            for x in xs:
                t += x
            means[g] = t / len(xs)
        return Series._of(self._d.keys(), [None if v is None or by.get(k) not in means
                                           else v - means[by[k]]
                                           for k, v in self._d.items()])  # fmt: skip

    def neutralize(self, *exposures):
        """What is left after regressing the values on `exposures` (Series, such as each
        market's beta to BTC) across markets, with an intercept: the part of the signal
        the exposures don't explain. None where any input is unknown."""
        keys = [k for k, v in self._d.items()
                if v is not None and all(e.get(k) is not None for e in exposures)]  # fmt: skip
        p = len(exposures) + 1
        if len(keys) <= p:
            return self._map(lambda _: None)
        rows = [[1.0] + [e[k] for e in exposures] for k in keys]
        y = [self._d[k] for k in keys]
        # Normal equations (X'X) b = X'y, solved by Gaussian elimination with pivoting.
        a = [[0.0] * (p + 1) for _ in range(p)]
        for r, yy in zip(rows, y):
            for i in range(p):
                for j in range(p):
                    a[i][j] += r[i] * r[j]
                a[i][p] += r[i] * yy
        for c in range(p):
            piv = max(range(c, p), key=lambda i: abs(a[i][c]))
            if a[piv][c] == 0:
                return self._map(lambda _: None)
            a[c], a[piv] = a[piv], a[c]
            for i in range(c + 1, p):
                f = a[i][c] / a[c][c]
                for j in range(c, p + 1):
                    a[i][j] -= f * a[c][j]
        b = [0.0] * p
        for i in range(p - 1, -1, -1):
            t = a[i][p]
            for j in range(i + 1, p):
                t -= a[i][j] * b[j]
            b[i] = t / a[i][i]
        out = dict.fromkeys(self._d)
        for k, r, yy in zip(keys, rows, y):
            fit = 0.0
            for i in range(p):
                fit += b[i] * r[i]
            out[k] = yy - fit
        return Series(out)

    def zscore(self):
        m, sd = self.mean(), self.std()
        if m is None or not sd:
            return self._map(lambda _: None)
        return self._map(lambda v: (v - m) / sd)

    def top(self, k):
        """The k largest known values, largest first (ties in universe order)."""
        known = [(k_, v) for k_, v in self._d.items() if v is not None]
        known.sort(key=lambda kv: -kv[1])
        return Series(known[:k])

    def bottom(self, k):
        """The k smallest known values, smallest first."""
        known = [(k_, v) for k_, v in self._d.items() if v is not None]
        known.sort(key=lambda kv: kv[1])
        return Series(known[:k])
