//! `oxarchive_backtest.math`: oxs-math behind Python's `math` signatures. Importing
//! `oxarchive_backtest` puts these into the standard `math` module (python/oxarchive_backtest/
//! _determinism.py), because `math` calls each platform's own libm and gets different bits on
//! macOS, Linux and in the browser.
//!
//! A number in gives a float out and raises exactly where CPython 3.14's `math` raises, with its
//! messages. A NumPy array (anything with `__array__` that is not a float or int) gives a float64
//! array out, computed element by element with NumPy's conventions: NaN and infinities, no
//! exceptions. Two arrays broadcast.

use oxs_math as m;
use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError, PyZeroDivisionError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyFloat, PyInt, PyTuple};

enum Arg<'py> {
    Scalar(f64),
    Array(Bound<'py, PyAny>),
}

fn is_array(x: &Bound<'_, PyAny>) -> PyResult<bool> {
    Ok(!x.is_instance_of::<PyFloat>() && !x.is_instance_of::<PyInt>() && x.hasattr("__array__")?)
}

fn arg<'py>(x: &Bound<'py, PyAny>) -> PyResult<Arg<'py>> {
    if is_array(x)? {
        return Ok(Arg::Array(x.clone()));
    }
    Ok(Arg::Scalar(x.extract()?))
}

fn repr(py: Python<'_>, x: f64) -> String {
    PyFloat::new(py, x).repr().map(|r| r.to_string()).unwrap_or_else(|_| x.to_string())
}

/// CPython's math_1: NaN from a non-NaN is a domain error; an infinity from a finite input is
/// an overflow (exp, cosh...) or a domain error (log(0), atanh(1)...).
fn check(py: Python<'_>, x: f64, r: f64, can_overflow: bool, domain: &str) -> PyResult<f64> {
    let domain_err = || {
        let msg = if domain.contains("%s") {
            domain.replace("%s", &repr(py, x))
        } else {
            domain.to_owned()
        };
        PyValueError::new_err(msg)
    };
    if r.is_nan() && !x.is_nan() {
        return Err(domain_err());
    }
    if r.is_infinite() && x.is_finite() {
        if can_overflow {
            return Err(PyOverflowError::new_err("math range error"));
        }
        return Err(domain_err());
    }
    Ok(r)
}

/// Applies `f` element by element to the arrays in `args`, broadcast together; float64 out.
fn map_arrays<'py>(
    py: Python<'py>,
    args: &[Bound<'py, PyAny>],
    f: impl Fn(&[f64]) -> f64,
) -> PyResult<Bound<'py, PyAny>> {
    let np = py.import("numpy")?;
    let kw = PyDict::new(py);
    kw.set_item("dtype", np.getattr("float64")?)?;
    let mut parts = Vec::with_capacity(args.len());
    let shape;
    if args.len() == 1 {
        let a = np.getattr("ascontiguousarray")?.call((&args[0],), Some(&kw))?;
        shape = a.getattr("shape")?;
        parts.push(PyBuffer::<f64>::get(&a)?.to_vec(py)?);
    } else {
        let tuple = PyTuple::new(py, args)?;
        let views = np.getattr("broadcast_arrays")?.call1(tuple)?;
        let mut s = None;
        for v in views.try_iter()? {
            let a = np.getattr("ascontiguousarray")?.call((v?,), Some(&kw))?;
            s = Some(a.getattr("shape")?);
            parts.push(PyBuffer::<f64>::get(&a)?.to_vec(py)?);
        }
        shape = s.expect("at least two arrays");
    }
    let n = parts.first().map_or(0, Vec::len);
    let mut row = vec![0.0; parts.len()];
    let out: Vec<f64> = (0..n)
        .map(|i| {
            for (k, p) in parts.iter().enumerate() {
                row[k] = p[i];
            }
            f(&row)
        })
        .collect();
    let arr = np.getattr("empty")?.call((shape,), Some(&kw))?;
    PyBuffer::<f64>::get(&arr)?.copy_from_slice(py, &out)?;
    Ok(arr)
}

fn float(py: Python<'_>, x: f64) -> Bound<'_, PyAny> {
    PyFloat::new(py, x).into_any()
}

macro_rules! unary {
    ($name:ident, $f:path, $over:expr, $domain:literal) => {
        #[doc = concat!("Deterministic math.", stringify!($name), "; arrays element by element.")]
        #[pyfunction]
        fn $name<'py>(py: Python<'py>, x: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
            match arg(x)? {
                Arg::Scalar(v) => Ok(float(py, check(py, v, $f(v), $over, $domain)?)),
                Arg::Array(a) => map_arrays(py, &[a], |v| $f(v[0])),
            }
        }
    };
}

unary!(acos, m::acos, false, "expected a number in range from -1 up to 1, got %s");
unary!(asin, m::asin, false, "expected a number in range from -1 up to 1, got %s");
unary!(acosh, m::acosh, false, "expected argument value not less than 1, got %s");
unary!(asinh, m::asinh, false, "math domain error");
unary!(atan, m::atan, false, "math domain error");
unary!(atanh, m::atanh, false, "expected a number between -1 and 1, got %s");
unary!(cbrt, m::cbrt, false, "math domain error");
unary!(cos, m::cos, false, "expected a finite input, got %s");
unary!(sin, m::sin, false, "expected a finite input, got %s");
unary!(tan, m::tan, false, "expected a finite input, got %s");
unary!(cosh, m::cosh, true, "math domain error");
unary!(sinh, m::sinh, true, "math domain error");
unary!(tanh, m::tanh, false, "math domain error");
unary!(exp, m::exp, true, "math domain error");
unary!(expm1, m::expm1, true, "math domain error");
unary!(log1p, m::ln_1p, false, "expected argument value > -1, got %s");

fn exp2_f(x: f64) -> f64 {
    m::pow(2.0, x)
}
unary!(exp2, exp2_f, true, "math domain error");

/// CPython's loghelper: ints of any size (a huge int is split as frac * 2**e first),
/// otherwise math_1 with "expected a positive input".
fn log_of(py: Python<'_>, x: &Bound<'_, PyAny>, f: fn(f64) -> f64) -> PyResult<f64> {
    if x.is_instance_of::<PyInt>() {
        if !x.gt(0)? {
            return Err(PyValueError::new_err("expected a positive input"));
        }
        return match x.extract::<f64>() {
            Ok(v) => Ok(f(v)),
            Err(e) if e.is_instance_of::<PyOverflowError>(py) => {
                // x ~= frac * 2**e with frac in [0.5, 1), rounded to nearest like _PyLong_Frexp:
                // the top 60 bits plus a sticky bit for the rest, rounded once to 53 bits.
                let mut e: i64 = x.call_method0("bit_length")?.extract()?;
                let shift = e - 60;
                let top = x.call_method1("__rshift__", (shift,))?;
                let sticky = top.call_method1("__lshift__", (shift,))?.ne(x)? as u64;
                let top: u64 = top.extract()?;
                let mut frac = (top | sticky) as f64 / (1u64 << 60) as f64;
                if frac == 1.0 {
                    frac = 0.5;
                    e += 1;
                }
                Ok(f(frac) + f(2.0) * e as f64)
            }
            Err(e) => Err(e),
        };
    }
    let v: f64 = x.extract()?;
    check(py, v, f(v), false, "expected a positive input, got %s")
}

/// Deterministic math.log(x[, base]); arrays element by element.
#[pyfunction]
#[pyo3(signature = (x, base=None))]
fn log<'py>(
    py: Python<'py>,
    x: &Bound<'py, PyAny>,
    base: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    if is_array(x)? || base.map(is_array).transpose()?.unwrap_or(false) {
        return match base {
            None => map_arrays(py, std::slice::from_ref(x), |v| m::ln(v[0])),
            Some(b) => map_arrays(py, &[x.clone(), b.clone()], |v| m::ln(v[0]) / m::ln(v[1])),
        };
    }
    let num = log_of(py, x, m::ln)?;
    let Some(base) = base else { return Ok(float(py, num)) };
    let den = log_of(py, base, m::ln)?;
    if den == 0.0 {
        return Err(PyZeroDivisionError::new_err("division by zero"));
    }
    Ok(float(py, num / den))
}

macro_rules! log_fn {
    ($name:ident, $f:path) => {
        #[doc = concat!("Deterministic math.", stringify!($name), "; arrays element by element.")]
        #[pyfunction]
        fn $name<'py>(py: Python<'py>, x: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
            if is_array(x)? {
                return map_arrays(py, std::slice::from_ref(x), |v| $f(v[0]));
            }
            Ok(float(py, log_of(py, x, $f)?))
        }
    };
}
log_fn!(log2, m::log2);
log_fn!(log10, m::log10);

/// x ** y with C99's special values (1 ** nan = 1, (-1) ** inf = 1), as CPython's math.pow
/// and NumPy have them; fdlibm for finite ** finite.
pub fn c_pow(x: f64, y: f64) -> f64 {
    if x.is_finite() && y.is_finite() {
        return m::pow(x, y);
    }
    if x.is_nan() {
        return if y == 0.0 { 1.0 } else { x };
    }
    if y.is_nan() {
        return if x == 1.0 { 1.0 } else { y };
    }
    if x.is_infinite() {
        let odd_y = y.is_finite() && y.abs() % 2.0 == 1.0;
        return if y > 0.0 {
            if odd_y { x } else { x.abs() }
        } else if y == 0.0 {
            1.0
        } else if odd_y {
            0.0f64.copysign(x)
        } else {
            0.0
        };
    }
    // y is infinite
    if x.abs() == 1.0 {
        1.0
    } else if (y > 0.0 && x.abs() > 1.0) || (y < 0.0 && x.abs() < 1.0) {
        f64::INFINITY
    } else {
        0.0
    }
}

/// Deterministic math.pow; arrays element by element and broadcast.
#[pyfunction]
fn pow<'py>(py: Python<'py>, x: &Bound<'py, PyAny>, y: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    match (arg(x)?, arg(y)?) {
        (Arg::Scalar(a), Arg::Scalar(b)) => {
            let r = c_pow(a, b);
            if a.is_finite() && b.is_finite() {
                if r.is_nan() || (r.is_infinite() && a == 0.0) {
                    return Err(PyValueError::new_err("math domain error"));
                }
                if r.is_infinite() {
                    return Err(PyOverflowError::new_err("math range error"));
                }
            }
            Ok(float(py, r))
        }
        _ => map_arrays(py, &[x.clone(), y.clone()], |v| c_pow(v[0], v[1])),
    }
}

/// Deterministic math.atan2; arrays element by element and broadcast.
#[pyfunction]
fn atan2<'py>(py: Python<'py>, y: &Bound<'py, PyAny>, x: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    match (arg(y)?, arg(x)?) {
        (Arg::Scalar(a), Arg::Scalar(b)) => Ok(float(py, m::atan2(a, b))),
        _ => map_arrays(py, &[y.clone(), x.clone()], |v| m::atan2(v[0], v[1])),
    }
}

/// Deterministic math.hypot(*coordinates); arrays element by element and broadcast.
#[pyfunction]
#[pyo3(signature = (*coords))]
fn hypot<'py>(py: Python<'py>, coords: &Bound<'py, PyTuple>) -> PyResult<Bound<'py, PyAny>> {
    let mut values = Vec::with_capacity(coords.len());
    let mut arrays = false;
    for c in coords.iter() {
        match arg(&c)? {
            Arg::Scalar(v) => values.push(v),
            Arg::Array(_) => arrays = true,
        }
    }
    if arrays {
        let all: Vec<_> = coords.iter().collect();
        return map_arrays(py, &all, m::hypot);
    }
    Ok(float(py, m::hypot(&values)))
}

/// Deterministic math.dist(p, q): hypot of the coordinate differences.
#[pyfunction]
fn dist<'py>(py: Python<'py>, p: &Bound<'py, PyAny>, q: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let p: Vec<f64> = p.extract().map_err(|_| PyTypeError::new_err("dist(): p must be a sequence of numbers"))?;
    let q: Vec<f64> = q.extract().map_err(|_| PyTypeError::new_err("dist(): q must be a sequence of numbers"))?;
    if p.len() != q.len() {
        return Err(PyValueError::new_err("both points must have the same number of dimensions"));
    }
    let d: Vec<f64> = p.iter().zip(&q).map(|(a, b)| a - b).collect();
    Ok(float(py, m::hypot(&d)))
}

/// CPython's float ** float (float_pow) on two floats: None where CPython would return a
/// complex number (negative ** non-integer), which the caller leaves to Python.
#[pyfunction]
fn float_pow(x: f64, y: f64) -> PyResult<Option<f64>> {
    if y == 0.0 {
        return Ok(Some(1.0));
    }
    if x.is_nan() {
        return Ok(Some(x));
    }
    if y.is_nan() {
        return Ok(Some(if x == 1.0 { 1.0 } else { y }));
    }
    let odd = |w: f64| w.is_finite() && w.abs() % 2.0 == 1.0;
    if y.is_infinite() {
        let ax = x.abs();
        return Ok(Some(if ax == 1.0 {
            1.0
        } else if (y > 0.0) == (ax > 1.0) {
            y.abs()
        } else {
            0.0
        }));
    }
    if x.is_infinite() {
        return Ok(Some(if y > 0.0 {
            if odd(y) { x } else { x.abs() }
        } else if odd(y) {
            0.0f64.copysign(x)
        } else {
            0.0
        }));
    }
    if x == 0.0 {
        if y < 0.0 {
            return Err(PyZeroDivisionError::new_err("zero to a negative power"));
        }
        return Ok(Some(if odd(y) { x } else { 0.0 }));
    }
    let mut ax = x;
    let mut negate = false;
    if x < 0.0 {
        if y != y.floor() {
            return Ok(None);
        }
        ax = -x;
        negate = odd(y);
    }
    if ax == 1.0 {
        return Ok(Some(if negate { -1.0 } else { 1.0 }));
    }
    let r = m::pow(ax, y);
    if r.is_infinite() {
        return Err(PyOverflowError::new_err((34, "Numerical result out of range")));
    }
    Ok(Some(if negate { -r } else { r }))
}

/// The cross-target math check's hash (oxs_math::check), as hex.
#[pyfunction]
fn check_hash() -> String {
    format!("{:016x}", m::check::combined_hash(m::check::TARGET_COUNT))
}

pub fn module<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyModule>> {
    let md = PyModule::new(py, "math")?;
    md.add_function(wrap_pyfunction!(acos, &md)?)?;
    md.add_function(wrap_pyfunction!(asin, &md)?)?;
    md.add_function(wrap_pyfunction!(acosh, &md)?)?;
    md.add_function(wrap_pyfunction!(asinh, &md)?)?;
    md.add_function(wrap_pyfunction!(atan, &md)?)?;
    md.add_function(wrap_pyfunction!(atanh, &md)?)?;
    md.add_function(wrap_pyfunction!(atan2, &md)?)?;
    md.add_function(wrap_pyfunction!(cbrt, &md)?)?;
    md.add_function(wrap_pyfunction!(cos, &md)?)?;
    md.add_function(wrap_pyfunction!(sin, &md)?)?;
    md.add_function(wrap_pyfunction!(tan, &md)?)?;
    md.add_function(wrap_pyfunction!(cosh, &md)?)?;
    md.add_function(wrap_pyfunction!(sinh, &md)?)?;
    md.add_function(wrap_pyfunction!(tanh, &md)?)?;
    md.add_function(wrap_pyfunction!(exp, &md)?)?;
    md.add_function(wrap_pyfunction!(exp2, &md)?)?;
    md.add_function(wrap_pyfunction!(expm1, &md)?)?;
    md.add_function(wrap_pyfunction!(log, &md)?)?;
    md.add_function(wrap_pyfunction!(log1p, &md)?)?;
    md.add_function(wrap_pyfunction!(log2, &md)?)?;
    md.add_function(wrap_pyfunction!(log10, &md)?)?;
    md.add_function(wrap_pyfunction!(pow, &md)?)?;
    md.add_function(wrap_pyfunction!(hypot, &md)?)?;
    md.add_function(wrap_pyfunction!(dist, &md)?)?;
    md.add_function(wrap_pyfunction!(float_pow, &md)?)?;
    md.add_function(wrap_pyfunction!(check_hash, &md)?)?;
    Ok(md)
}
