//! The math functions IEEE 754 does not pin down, written in plain f64 arithmetic so every
//! build target (native, WebAssembly, the Python package) gives the same bits: exp, expm1, ln, ln_1p,
//! log2, log10, pow, sin, cos, tan, asin, acos, atan, atan2, sinh, cosh, tanh, asinh, acosh,
//! atanh, cbrt and hypot.
//!
//! This is fdlibm as V8 has it in src/base/ieee754.cc, computed without fused multiply-adds. So these
//! give the same results as 0xArchive's other script engines bit for bit; `check` holds the reference input
//! generator and the hashes its tests pin (x64 V8's own Math over the same inputs).
//!
//! Rules for this file: + - * / and the exactly rounded `sqrt`, `abs` and `floor` only, never
//! `mul_add` or the platform's libm. Rust never fuses a multiply and an add on its own. Integer
//! steps are i32 with wrapping where the C code relies on two's complement.
//!
//! Sun's notices, kept as the source files carry them:
//!
//! ====================================================
//! Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//!
//! Developed at SunSoft, a Sun Microsystems, Inc. business.
//! Permission to use, copy, modify, and distribute this
//! software is freely granted, provided that this notice
//! is preserved.
//! ====================================================
//!
//! (e_exp.c, e_pow.c, s_expm1.c:)
//! ====================================================
//! Copyright (C) 2004 by Sun Microsystems, Inc. All rights reserved.
//!
//! Permission to use, copy, modify, and distribute this
//! software is freely granted, provided that this notice
//! is preserved.
//! ====================================================
//!
//! (k_tan.c:)
//! ====================================================
//! Copyright 2004 Sun Microsystems, Inc.  All Rights Reserved.
//!
//! Permission to use, copy, modify, and distribute this
//! software is freely granted, provided that this notice
//! is preserved.
//! ====================================================
//!
//! V8's port, whose departures this follows: "The original source code covered by the above
//! license above has been modified significantly by Google Inc. Copyright 2016 the V8 project
//! authors. All rights reserved." V8 is BSD-licensed.

// The port stays line for line with the C source so it can be checked against it: constants as
// written there, `x - x` for NaN, its loop counters and `y = y * t`.
#![allow(
    clippy::excessive_precision,
    clippy::unreadable_literal,
    clippy::approx_constant,
    clippy::eq_op,
    clippy::explicit_counter_loop,
    clippy::assign_op_pattern
)]

pub mod check;

// ---------------------------------------------------------------------------------------------
// Word access: the high word signed (fdlibm's int32_t hx), the low word unsigned (uint32_t lx).
// ---------------------------------------------------------------------------------------------

#[inline]
fn high(x: f64) -> i32 {
    (x.to_bits() >> 32) as i32
}

#[inline]
fn low(x: f64) -> u32 {
    x.to_bits() as u32
}

#[inline]
fn words(hi: i32, lo: u32) -> f64 {
    f64::from_bits(((hi as u32 as u64) << 32) | lo as u64)
}

#[inline]
fn with_high(x: f64, hi: i32) -> f64 {
    words(hi, low(x))
}

#[inline]
fn with_low(x: f64, lo: u32) -> f64 {
    words(high(x), lo)
}

const HUGE: f64 = 1.0e300;
const TINY: f64 = 1.0e-300;
const TWO24: f64 = 1.67772160000000000000e+07;
const TWON24: f64 = 5.96046447753906250000e-08;
const TWO53: f64 = 9007199254740992.0;
const TWO54: f64 = 1.80143985094819840000e+16;
const TWOM54: f64 = 5.55111512312578270212e-17;
const SIGN: i32 = i32::MIN;

fn copysign(x: f64, y: f64) -> f64 {
    with_high(x, (high(x) & 0x7fffffff) | (high(y) & SIGN))
}

/// s_scalbn.c: x * 2**n by exponent arithmetic; one rounding when the result is subnormal.
fn scalbn(mut x: f64, n: i32) -> f64 {
    const KEEP: i32 = 0x800fffffu32 as i32;
    let mut hx = high(x);
    let lx = low(x);
    let mut k = (hx & 0x7ff00000) >> 20;
    if k == 0 {
        if (lx | (hx & 0x7fffffff) as u32) == 0 {
            return x; /* +-0 */
        }
        x *= TWO54;
        hx = high(x);
        k = ((hx & 0x7ff00000) >> 20) - 54;
        if n < -50000 {
            return TINY * x; /* underflow */
        }
    }
    if k == 0x7ff {
        return x + x; /* NaN or Inf */
    }
    k = k.wrapping_add(n);
    if k > 0x7fe {
        return HUGE * copysign(HUGE, x); /* overflow */
    }
    if k > 0 {
        return with_high(x, (hx & KEEP) | (k << 20)); /* normal result */
    }
    if k <= -54 {
        if n > 50000 {
            return HUGE * copysign(HUGE, x); /* overflow */
        }
        return TINY * copysign(TINY, x); /* underflow */
    }
    k += 54; /* subnormal result */
    with_high(x, (hx & KEEP) | (k << 20)) * TWOM54
}

// ---------------------------------------------------------------------------------------------
// Argument reduction and the kernels on [-pi/4, pi/4] (e_rem_pio2.c, k_rem_pio2.c, k_sin.c,
// k_cos.c, k_tan.c).
// ---------------------------------------------------------------------------------------------

/// 2/pi in 24-bit pieces: 396 hex digits (476 decimal).
const TWO_OVER_PI: [i32; 66] = [
    0xA2F983, 0x6E4E44, 0x1529FC, 0x2757D1, 0xF534DD, 0xC0DB62, 0x95993C, 0x439041, 0xFE5163,
    0xABDEBB, 0xC561B7, 0x246E3A, 0x424DD2, 0xE00649, 0x2EEA09, 0xD1921C, 0xFE1DEB, 0x1CB129,
    0xA73EE8, 0x8235F5, 0x2EBB44, 0x84E99C, 0x7026B4, 0x5F7E41, 0x3991D6, 0x398353, 0x39F49C,
    0x845F8B, 0xBDF928, 0x3B1FF8, 0x97FFDE, 0x05980F, 0xEF2F11, 0x8B5A0A, 0x6D1F6D, 0x367ECF,
    0x27CB09, 0xB74F46, 0x3F669E, 0x5FEA2D, 0x7527BA, 0xC7EBE5, 0xF17B3D, 0x0739F7, 0x8A5292,
    0xEA6BFB, 0x5FB11F, 0x8D5D08, 0x560330, 0x46FC7B, 0x6BABF0, 0xCFBC20, 0x9AF436, 0x1DA9E3,
    0x91615E, 0xE61B08, 0x659985, 0x5F14A0, 0x68408D, 0xFFD880, 0x4D7327, 0x310606, 0x1556CA,
    0x73A8C9, 0x60E27B, 0xC08C6B,
];

const NPIO2_HW: [i32; 32] = [
    0x3FF921FB, 0x400921FB, 0x4012D97C, 0x401921FB, 0x401F6A7A, 0x4022D97C, 0x4025FDBB,
    0x402921FB, 0x402C463A, 0x402F6A7A, 0x4031475C, 0x4032D97C, 0x40346B9C, 0x4035FDBB,
    0x40378FDB, 0x403921FB, 0x403AB41B, 0x403C463A, 0x403DD85A, 0x403F6A7A, 0x40407E4C,
    0x4041475C, 0x4042106C, 0x4042D97C, 0x4043A28C, 0x40446B9C, 0x404534AC, 0x4045FDBB,
    0x4046C6CB, 0x40478FDB, 0x404858EB, 0x404921FB,
];

const INVPIO2: f64 = 6.36619772367581382433e-01;
const PIO2_1: f64 = 1.57079632673412561417e+00;
const PIO2_1T: f64 = 6.07710050650619224932e-11;
const PIO2_2: f64 = 6.07710050630396597660e-11;
const PIO2_2T: f64 = 2.02226624879595063154e-21;
const PIO2_3: f64 = 2.02226624871116645580e-21;
const PIO2_3T: f64 = 8.47842766036889956997e-32;

/// pi/2 cut into 24-bit pieces (k_rem_pio2.c PIo2).
const PIO2_PIECES: [f64; 8] = [
    1.57079625129699707031e+00,
    7.54978941586159635335e-08,
    5.39030252995776476554e-15,
    3.28200341580791294123e-22,
    1.27065575308067607349e-29,
    1.22933308981111328932e-36,
    2.73370053816464559624e-44,
    2.16741683877804819444e-51,
];

/// __kernel_rem_pio2 for prec = 2, the only precision __ieee754_rem_pio2 asks for (jk = 4).
/// x[0..nx] are 24-bit pieces of |x| scaled by 2**-e0; returns the last three bits of N and
/// x - N*pi/2 as y0 + y1.
fn kernel_rem_pio2(x: &[f64; 3], e0: i32, nx: i32) -> (i32, f64, f64) {
    const JK: i32 = 4;
    const JP: i32 = JK;
    let mut iq = [0i32; 20];
    let mut f = [0f64; 20];
    let mut fq = [0f64; 20];
    let mut q = [0f64; 20];
    let u = |i: i32| i as usize;

    let jx = nx - 1;
    let mut jv = (e0 - 3) / 24;
    if jv < 0 {
        jv = 0;
    }
    let mut q0 = e0 - 24 * (jv + 1);

    let mut j = jv - jx;
    for i in 0..=jx + JK {
        f[u(i)] = if j < 0 { 0.0 } else { TWO_OVER_PI[u(j)] as f64 };
        j += 1;
    }
    for i in 0..=JK {
        let mut fw = 0.0;
        for j in 0..=jx {
            fw += x[u(j)] * f[u(jx + i - j)];
        }
        q[u(i)] = fw;
    }

    let mut jz = JK;
    let mut z;
    let mut n;
    let mut ih;
    loop {
        /* distill q[] into iq[] reversingly */
        let mut i = 0;
        let mut j = jz;
        z = q[u(jz)];
        while j > 0 {
            let fw = ((TWON24 * z) as i32) as f64;
            iq[u(i)] = (z - TWO24 * fw) as i32;
            z = q[u(j - 1)] + fw;
            i += 1;
            j -= 1;
        }

        /* compute n */
        z = scalbn(z, q0); /* actual value of z */
        z -= 8.0 * (z * 0.125).floor(); /* trim off integer >= 8 */
        n = z as i32;
        z -= n as f64;
        ih = 0;
        if q0 > 0 {
            /* need iq[jz-1] to determine n */
            let i = iq[u(jz - 1)] >> (24 - q0);
            n += i;
            iq[u(jz - 1)] -= i << (24 - q0);
            ih = iq[u(jz - 1)] >> (23 - q0);
        } else if q0 == 0 {
            ih = iq[u(jz - 1)] >> 23;
        } else if z >= 0.5 {
            ih = 2;
        }

        if ih > 0 {
            /* q > 0.5 */
            n += 1;
            let mut carry = 0;
            for i in 0..jz {
                /* compute 1-q */
                let j = iq[u(i)];
                if carry == 0 {
                    if j != 0 {
                        carry = 1;
                        iq[u(i)] = 0x1000000 - j;
                    }
                } else {
                    iq[u(i)] = 0xffffff - j;
                }
            }
            if q0 > 0 {
                /* rare case: chance is 1 in 12 */
                if q0 == 1 {
                    iq[u(jz - 1)] &= 0x7fffff;
                } else if q0 == 2 {
                    iq[u(jz - 1)] &= 0x3fffff;
                }
            }
            if ih == 2 {
                z = 1.0 - z;
                if carry != 0 {
                    z -= scalbn(1.0, q0);
                }
            }
        }

        /* check if recomputation is needed */
        if z == 0.0 {
            let mut j = 0;
            let mut i = jz - 1;
            while i >= JK {
                j |= iq[u(i)];
                i -= 1;
            }
            if j == 0 {
                /* need recomputation */
                let mut k = 1;
                while JK >= k && iq[u(JK - k)] == 0 {
                    k += 1; /* k = no. of terms needed */
                }
                for i in jz + 1..=jz + k {
                    /* add q[jz+1] to q[jz+k] */
                    f[u(jx + i)] = TWO_OVER_PI[u(jv + i)] as f64;
                    let mut fw = 0.0;
                    for j in 0..=jx {
                        fw += x[u(j)] * f[u(jx + i - j)];
                    }
                    q[u(i)] = fw;
                }
                jz += k;
                continue; /* goto recompute */
            }
        }
        break;
    }

    /* chop off zero terms */
    if z == 0.0 {
        jz -= 1;
        q0 -= 24;
        while iq[u(jz)] == 0 {
            jz -= 1;
            q0 -= 24;
        }
    } else {
        /* break z into 24-bit if necessary */
        z = scalbn(z, -q0);
        if z >= TWO24 {
            let fw = ((TWON24 * z) as i32) as f64;
            iq[u(jz)] = (z - TWO24 * fw) as i32;
            jz += 1;
            q0 += 24;
            iq[u(jz)] = fw as i32;
        } else {
            iq[u(jz)] = z as i32;
        }
    }

    /* convert integer "bit" chunk to floating-point value */
    let mut fw = scalbn(1.0, q0);
    let mut i = jz;
    while i >= 0 {
        q[u(i)] = fw * iq[u(i)] as f64;
        fw *= TWON24;
        i -= 1;
    }

    /* compute PIo2[0,...,jp]*q[jz,...,0] */
    let mut i = jz;
    while i >= 0 {
        let mut fw = 0.0;
        let mut k = 0;
        while k <= JP && k <= jz - i {
            fw += PIO2_PIECES[u(k)] * q[u(i + k)];
            k += 1;
        }
        fq[u(jz - i)] = fw;
        i -= 1;
    }

    /* compress fq[] into y[] (prec 1 and 2) */
    let mut fw = 0.0;
    let mut i = jz;
    while i >= 0 {
        fw += fq[u(i)];
        i -= 1;
    }
    let y0 = if ih == 0 { fw } else { -fw };
    fw = fq[0] - fw;
    for i in 1..=jz {
        fw += fq[u(i)];
    }
    let y1 = if ih == 0 { fw } else { -fw };
    (n & 7, y0, y1)
}

/// __ieee754_rem_pio2: returns N (its low bits for huge x) and x - N*pi/2 as y0 + y1.
fn rem_pio2(x: f64) -> (i32, f64, f64) {
    let hx = high(x);
    let ix = hx & 0x7fffffff;
    if ix <= 0x3fe921fb {
        /* |x| ~<= pi/4 , no need for reduction */
        return (0, x, 0.0);
    }
    if ix < 0x4002d97c {
        /* |x| < 3pi/4, special case with n=+-1 */
        if hx > 0 {
            let mut z = x - PIO2_1;
            let (y0, y1);
            if ix != 0x3ff921fb {
                /* 33+53 bit pi is good enough */
                y0 = z - PIO2_1T;
                y1 = (z - y0) - PIO2_1T;
            } else {
                /* near pi/2, use 33+33+53 bit pi */
                z -= PIO2_2;
                y0 = z - PIO2_2T;
                y1 = (z - y0) - PIO2_2T;
            }
            return (1, y0, y1);
        }
        let mut z = x + PIO2_1;
        let (y0, y1);
        if ix != 0x3ff921fb {
            y0 = z + PIO2_1T;
            y1 = (z - y0) + PIO2_1T;
        } else {
            z += PIO2_2;
            y0 = z + PIO2_2T;
            y1 = (z - y0) + PIO2_2T;
        }
        return (-1, y0, y1);
    }
    if ix <= 0x413921fb {
        /* |x| ~<= 2^19*(pi/2), medium size */
        let mut t = x.abs();
        let n = (t * INVPIO2 + 0.5) as i32;
        let fn_ = n as f64;
        let mut r = t - fn_ * PIO2_1;
        let mut w = fn_ * PIO2_1T; /* 1st round good to 85 bit */
        let mut y0;
        if n < 32 && ix != NPIO2_HW[(n - 1) as usize] {
            y0 = r - w; /* quick check no cancellation */
        } else {
            let j = ix >> 20;
            y0 = r - w;
            let mut i = j - ((high(y0) >> 20) & 0x7ff);
            if i > 16 {
                /* 2nd iteration needed, good to 118 */
                t = r;
                w = fn_ * PIO2_2;
                r = t - w;
                w = fn_ * PIO2_2T - ((t - r) - w);
                y0 = r - w;
                i = j - ((high(y0) >> 20) & 0x7ff);
                if i > 49 {
                    /* 3rd iteration need, 151 bits acc */
                    t = r;
                    w = fn_ * PIO2_3;
                    r = t - w;
                    w = fn_ * PIO2_3T - ((t - r) - w);
                    y0 = r - w;
                }
            }
        }
        let y1 = (r - y0) - w;
        if hx < 0 {
            return (-n, -y0, -y1);
        }
        return (n, y0, y1);
    }
    /* all other (large) arguments */
    if ix >= 0x7ff00000 {
        /* x is inf or NaN */
        let nan = x - x;
        return (0, nan, nan);
    }
    /* set z = scalbn(|x|,ilogb(x)-23) */
    let e0 = (ix >> 20) - 1046; /* e0 = ilogb(z)-23; */
    let mut z = words(ix - (e0 << 20), low(x));
    let mut tx = [0f64; 3];
    for v in tx.iter_mut().take(2) {
        *v = (z as i32) as f64;
        z = (z - *v) * TWO24;
    }
    tx[2] = z;
    let mut nx = 3;
    while tx[nx - 1] == 0.0 {
        nx -= 1; /* skip zero term */
    }
    let (n, y0, y1) = kernel_rem_pio2(&tx, e0, nx as i32);
    if hx < 0 {
        return (-n, -y0, -y1);
    }
    (n, y0, y1)
}

const C1: f64 = 4.16666666666666019037e-02;
const C2: f64 = -1.38888888888741095749e-03;
const C3: f64 = 2.48015872894767294178e-05;
const C4: f64 = -2.75573143513906633035e-07;
const C5: f64 = 2.08757232129817482790e-09;
const C6: f64 = -1.13596475577881948265e-11;

/// __kernel_cos(x, y): cos(x + y) for |x| ~<= pi/4, y the tail of x.
fn kernel_cos(x: f64, y: f64) -> f64 {
    let ix = high(x) & 0x7fffffff;
    // fdlibm tests (int)x == 0 here to raise inexact; for |x| < 2**-27 it always holds.
    if ix < 0x3e400000 {
        return 1.0;
    }
    let z = x * x;
    let r = z * (C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6)))));
    if ix < 0x3fd33333 {
        return 1.0 - (0.5 * z - (z * r - x * y)); /* if |x| < 0.3 */
    }
    let qx = if ix > 0x3fe90000 { 0.28125 } else { words(ix - 0x00200000, 0) }; /* x/4 */
    let iz = 0.5 * z - qx;
    let a = 1.0 - qx;
    a - (iz - (z * r - x * y))
}

const S1: f64 = -1.66666666666666324348e-01;
const S2: f64 = 8.33333333332248946124e-03;
const S3: f64 = -1.98412698298579493134e-04;
const S4: f64 = 2.75573137070700676789e-06;
const S5: f64 = -2.50507602534068634195e-08;
const S6: f64 = 1.58969099521155010221e-10;

/// __kernel_sin(x, y, iy): sin(x + y) for |x| ~<= pi/4; iy = 0 means y is zero.
fn kernel_sin(x: f64, y: f64, iy: i32) -> f64 {
    let ix = high(x) & 0x7fffffff;
    if ix < 0x3e400000 {
        return x; /* |x| < 2**-27: (int)x == 0 always holds */
    }
    let z = x * x;
    let v = z * x;
    let r = S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)));
    if iy == 0 {
        return x + v * (S1 + z * r);
    }
    x - ((z * (0.5 * y - v * r) - y) - v * S1)
}

const T: [f64; 13] = [
    3.33333333333334091986e-01,
    1.33333333333201242699e-01,
    5.39682539762260521377e-02,
    2.18694882948595424599e-02,
    8.86323982359930005737e-03,
    3.59207910759131235356e-03,
    1.45620945432529025516e-03,
    5.88041240820264096874e-04,
    2.46463134818469906812e-04,
    7.81794442939557092300e-05,
    7.14072491382608190305e-05,
    -1.85586374855275456654e-05,
    2.59073051863633712884e-05,
];
const PIO4: f64 = 7.85398163397448278999e-01;
const PIO4LO: f64 = 3.06161699786838301793e-17;

/// __kernel_tan(x, y, iy): tan(x + y) if iy = 1, -1/tan(x + y) if iy = -1, for |x| ~<= pi/4.
fn kernel_tan(mut x: f64, mut y: f64, iy: i32) -> f64 {
    let hx = high(x);
    let ix = hx & 0x7fffffff;
    if ix < 0x3e300000 {
        /* x < 2**-28; (int)x == 0 always holds */
        if ((ix as u32 | low(x)) | (iy + 1) as u32) == 0 {
            return 1.0 / x.abs();
        }
        if iy == 1 {
            return x;
        }
        /* compute -1 / (x+y) carefully */
        let w = x + y;
        let z = with_low(w, 0);
        let v = y - (z - x);
        let a = -1.0 / w;
        let t = with_low(a, 0);
        let s = 1.0 + t * z;
        return t + a * (s + t * v);
    }
    if ix >= 0x3fe59428 {
        /* |x| >= 0.6744 */
        if hx < 0 {
            x = -x;
            y = -y;
        }
        let z = PIO4 - x;
        let w = PIO4LO - y;
        x = z + w;
        y = 0.0;
    }
    let z = x * x;
    let w = z * z;
    /* Break x^5*(T[1]+x^2*T[2]+...) into odd and even parts */
    let mut r = T[1] + w * (T[3] + w * (T[5] + w * (T[7] + w * (T[9] + w * T[11]))));
    let v = z * (T[2] + w * (T[4] + w * (T[6] + w * (T[8] + w * (T[10] + w * T[12])))));
    let s = z * x;
    r = y + z * (s * (r + v) + y);
    r += T[0] * s;
    let w = x + r;
    if ix >= 0x3fe59428 {
        let v = iy as f64;
        return (1 - ((hx >> 30) & 2)) as f64 * (v - 2.0 * (x - (w * w / (w + v) - r)));
    }
    if iy == 1 {
        return w;
    }
    /* compute -1.0 / (x+r) accurately */
    let z = with_low(w, 0);
    let v = r - (z - x); /* z+v = r+x */
    let a = -1.0 / w;
    let t = with_low(a, 0);
    let s = 1.0 + t * z;
    t + a * (s + t * v)
}

// ---------------------------------------------------------------------------------------------
// The public functions.
// ---------------------------------------------------------------------------------------------

/// s_sin.c
pub fn sin(x: f64) -> f64 {
    let ix = high(x) & 0x7fffffff;
    if ix <= 0x3fe921fb {
        return kernel_sin(x, 0.0, 0);
    }
    if ix >= 0x7ff00000 {
        return x - x; /* sin(Inf or NaN) is NaN */
    }
    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => kernel_sin(y0, y1, 1),
        1 => kernel_cos(y0, y1),
        2 => -kernel_sin(y0, y1, 1),
        _ => -kernel_cos(y0, y1),
    }
}

/// s_cos.c
pub fn cos(x: f64) -> f64 {
    let ix = high(x) & 0x7fffffff;
    if ix <= 0x3fe921fb {
        return kernel_cos(x, 0.0);
    }
    if ix >= 0x7ff00000 {
        return x - x;
    }
    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => kernel_cos(y0, y1),
        1 => -kernel_sin(y0, y1, 1),
        2 => -kernel_cos(y0, y1),
        _ => kernel_sin(y0, y1, 1),
    }
}

/// s_tan.c
pub fn tan(x: f64) -> f64 {
    let ix = high(x) & 0x7fffffff;
    if ix <= 0x3fe921fb {
        return kernel_tan(x, 0.0, 1);
    }
    if ix >= 0x7ff00000 {
        return x - x;
    }
    let (n, y0, y1) = rem_pio2(x);
    kernel_tan(y0, y1, 1 - ((n & 1) << 1)) /* 1 -> n even, -1 -> n odd */
}

const PI: f64 = 3.14159265358979311600e+00;
const PIO2_HI: f64 = 1.57079632679489655800e+00;
const PIO2_LO: f64 = 6.12323399573676603587e-17;
const PIO4_HI: f64 = 7.85398163397448278999e-01;
const PS0: f64 = 1.66666666666666657415e-01;
const PS1: f64 = -3.25565818622400915405e-01;
const PS2: f64 = 2.01212532134862925881e-01;
const PS3: f64 = -4.00555345006794114027e-02;
const PS4: f64 = 7.91534994289814532176e-04;
const PS5: f64 = 3.47933107596021167570e-05;
const QS1: f64 = -2.40339491173441421878e+00;
const QS2: f64 = 2.02094576023350569471e+00;
const QS3: f64 = -6.88283971605453293030e-01;
const QS4: f64 = 7.70381505559019352791e-02;

/// e_acos.c
pub fn acos(x: f64) -> f64 {
    let hx = high(x);
    let ix = hx & 0x7fffffff;
    if ix >= 0x3ff00000 {
        /* |x| >= 1 */
        if ((ix - 0x3ff00000) as u32 | low(x)) == 0 {
            /* |x|==1 */
            if hx > 0 {
                return 0.0; /* acos(1) = 0 */
            }
            return PI + 2.0 * PIO2_LO; /* acos(-1)= pi */
        }
        return f64::NAN; /* acos(|x|>1) is NaN */
    }
    if ix < 0x3fe00000 {
        /* |x| < 0.5 */
        if ix <= 0x3c600000 {
            return PIO2_HI + PIO2_LO; /* if |x|<2**-57 */
        }
        let z = x * x;
        let p = z * (PS0 + z * (PS1 + z * (PS2 + z * (PS3 + z * (PS4 + z * PS5)))));
        let q = 1.0 + z * (QS1 + z * (QS2 + z * (QS3 + z * QS4)));
        let r = p / q;
        return PIO2_HI - (x - (PIO2_LO - x * r));
    }
    if hx < 0 {
        /* x < -0.5 */
        let z = (1.0 + x) * 0.5;
        let p = z * (PS0 + z * (PS1 + z * (PS2 + z * (PS3 + z * (PS4 + z * PS5)))));
        let q = 1.0 + z * (QS1 + z * (QS2 + z * (QS3 + z * QS4)));
        let s = z.sqrt();
        let r = p / q;
        let w = r * s - PIO2_LO;
        return PI - 2.0 * (s + w);
    }
    /* x > 0.5 */
    let z = (1.0 - x) * 0.5;
    let s = z.sqrt();
    let df = with_low(s, 0);
    let c = (z - df * df) / (s + df);
    let p = z * (PS0 + z * (PS1 + z * (PS2 + z * (PS3 + z * (PS4 + z * PS5)))));
    let q = 1.0 + z * (QS1 + z * (QS2 + z * (QS3 + z * QS4)));
    let r = p / q;
    let w = r * s + c;
    2.0 * (df + w)
}

/// e_asin.c
pub fn asin(x: f64) -> f64 {
    let hx = high(x);
    let ix = hx & 0x7fffffff;
    if ix >= 0x3ff00000 {
        /* |x|>= 1 */
        if ((ix - 0x3ff00000) as u32 | low(x)) == 0 {
            return x * PIO2_HI + x * PIO2_LO; /* asin(1)=+-pi/2 */
        }
        return f64::NAN; /* asin(|x|>1) is NaN */
    }
    if ix < 0x3fe00000 {
        /* |x|<0.5 */
        if ix < 0x3e400000 {
            return x; /* |x| < 2**-27 */
        }
        let t = x * x;
        let p = t * (PS0 + t * (PS1 + t * (PS2 + t * (PS3 + t * (PS4 + t * PS5)))));
        let q = 1.0 + t * (QS1 + t * (QS2 + t * (QS3 + t * QS4)));
        let w = p / q;
        return x + x * w;
    }
    /* 1> |x|>= 0.5 */
    let w = 1.0 - x.abs();
    let mut t = w * 0.5;
    let mut p = t * (PS0 + t * (PS1 + t * (PS2 + t * (PS3 + t * (PS4 + t * PS5)))));
    let mut q = 1.0 + t * (QS1 + t * (QS2 + t * (QS3 + t * QS4)));
    let s = t.sqrt();
    if ix >= 0x3fef3333 {
        /* if |x| > 0.975 */
        let w = p / q;
        t = PIO2_HI - (2.0 * (s + s * w) - PIO2_LO);
    } else {
        let w = with_low(s, 0);
        let c = (t - w * w) / (s + w);
        let r = p / q;
        p = 2.0 * s * r - (PIO2_LO - 2.0 * c);
        q = PIO4_HI - 2.0 * w;
        t = PIO4_HI - (p - q);
    }
    if hx > 0 { t } else { -t }
}

const ATANHI: [f64; 4] = [
    4.63647609000806093515e-01,
    7.85398163397448278999e-01,
    9.82793723247329054082e-01,
    1.57079632679489655800e+00,
];
const ATANLO: [f64; 4] = [
    2.26987774529616870924e-17,
    3.06161699786838301793e-17,
    1.39033110312309984516e-17,
    6.12323399573676603587e-17,
];
const AT: [f64; 11] = [
    3.33333333333329318027e-01,
    -1.99999999998764832476e-01,
    1.42857142725034663711e-01,
    -1.11111104054623557880e-01,
    9.09088713343650656196e-02,
    -7.69187620504482999495e-02,
    6.66107313738753120669e-02,
    -5.83357013379057348645e-02,
    4.97687799461593236017e-02,
    -3.65315727442169155270e-02,
    1.62858201153657823623e-02,
];

/// s_atan.c
pub fn atan(mut x: f64) -> f64 {
    let hx = high(x);
    let ix = hx & 0x7fffffff;
    let id: i32;
    if ix >= 0x44100000 {
        /* if |x| >= 2^66 */
        if ix > 0x7ff00000 || (ix == 0x7ff00000 && low(x) != 0) {
            return x + x; /* NaN */
        }
        if hx > 0 {
            return ATANHI[3] + ATANLO[3];
        }
        return -ATANHI[3] - ATANLO[3];
    }
    if ix < 0x3fdc0000 {
        /* |x| < 0.4375 */
        if ix < 0x3e400000 {
            return x; /* |x| < 2^-27 */
        }
        id = -1;
    } else {
        x = x.abs();
        if ix < 0x3ff30000 {
            /* |x| < 1.1875 */
            if ix < 0x3fe60000 {
                /* 7/16 <=|x|<11/16 */
                id = 0;
                x = (2.0 * x - 1.0) / (2.0 + x);
            } else {
                /* 11/16<=|x|< 19/16 */
                id = 1;
                x = (x - 1.0) / (x + 1.0);
            }
        } else if ix < 0x40038000 {
            /* |x| < 2.4375 */
            id = 2;
            x = (x - 1.5) / (1.0 + 1.5 * x);
        } else {
            /* 2.4375 <= |x| < 2^66 */
            id = 3;
            x = -1.0 / x;
        }
    }
    /* end of argument reduction */
    let z = x * x;
    let w = z * z;
    /* break sum from i=0 to 10 aT[i]z**(i+1) into odd and even poly */
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));
    if id < 0 {
        return x - x * (s1 + s2);
    }
    let id = id as usize;
    let z = ATANHI[id] - ((x * (s1 + s2) - ATANLO[id]) - x);
    if hx < 0 { -z } else { z }
}

const PI_O_4: f64 = 7.8539816339744827900e-01;
const PI_O_2: f64 = 1.5707963267948965580e+00;
const PI_LO: f64 = 1.2246467991473531772e-16;

/// e_atan2.c
pub fn atan2(y: f64, x: f64) -> f64 {
    let hx = high(x);
    let lx = low(x);
    let ix = hx & 0x7fffffff;
    let hy = high(y);
    let ly = low(y);
    let iy = hy & 0x7fffffff;
    if (ix as u32 | ((lx | lx.wrapping_neg()) >> 31)) > 0x7ff00000
        || (iy as u32 | ((ly | ly.wrapping_neg()) >> 31)) > 0x7ff00000
    {
        return x + y; /* x or y is NaN */
    }
    if (hx.wrapping_sub(0x3ff00000) as u32 | lx) == 0 {
        return atan(y); /* x=1.0 */
    }
    let mut m = ((hy >> 31) & 1) | ((hx >> 30) & 2); /* 2*sign(x)+sign(y) */

    /* when y = 0 */
    if (iy as u32 | ly) == 0 {
        return match m {
            0 | 1 => y,         /* atan(+-0,+anything)=+-0 */
            2 => PI + TINY,     /* atan(+0,-anything) = pi */
            _ => -PI - TINY,    /* atan(-0,-anything) =-pi */
        };
    }
    /* when x = 0 */
    if (ix as u32 | lx) == 0 {
        return if hy < 0 { -PI_O_2 - TINY } else { PI_O_2 + TINY };
    }

    /* when x is INF */
    if ix == 0x7ff00000 {
        if iy == 0x7ff00000 {
            return match m {
                0 => PI_O_4 + TINY,
                1 => -PI_O_4 - TINY,
                2 => 3.0 * PI_O_4 + TINY,
                _ => -3.0 * PI_O_4 - TINY,
            };
        }
        return match m {
            0 => 0.0,
            1 => -0.0,
            2 => PI + TINY,
            _ => -PI - TINY,
        };
    }
    /* when y is INF */
    if iy == 0x7ff00000 {
        return if hy < 0 { -PI_O_2 - TINY } else { PI_O_2 + TINY };
    }

    /* compute y/x */
    let k = (iy - ix) >> 20;
    let z;
    if k > 60 {
        /* |y/x| >  2**60 */
        z = PI_O_2 + 0.5 * PI_LO;
        m &= 1;
    } else if hx < 0 && k < -60 {
        z = 0.0; /* 0 > |y|/x > -2**-60 */
    } else {
        z = atan((y / x).abs()); /* safe to do y/x */
    }
    match m {
        0 => z,                  /* atan(+,+) */
        1 => -z,                 /* atan(-,+) */
        2 => PI - (z - PI_LO),   /* atan(+,-) */
        _ => (z - PI_LO) - PI,   /* atan(-,-) */
    }
}

const LN2_HI: f64 = 6.93147180369123816490e-01;
const LN2_LO: f64 = 1.90821492927058770002e-10;
const INVLN2: f64 = 1.44269504088896338700e+00;
const O_THRESHOLD: f64 = 7.09782712893383973096e+02;
const U_THRESHOLD: f64 = -7.45133219101941108420e+02;
const HALF: [f64; 2] = [0.5, -0.5];
const LN2HI: [f64; 2] = [6.93147180369123816490e-01, -6.93147180369123816490e-01];
const LN2LO: [f64; 2] = [1.90821492927058770002e-10, -1.90821492927058770002e-10];
const P1: f64 = 1.66666666666666019037e-01;
const P2: f64 = -2.77777777770155933842e-03;
const P3: f64 = 6.61375632143793436117e-05;
const P4: f64 = -1.65339022054652515390e-06;
const P5: f64 = 4.13813679705723846039e-08;
const E: f64 = 2.718281828459045;
const TWOM1000: f64 = 9.33263618503218878990e-302;
const TWO1023: f64 = 8.988465674311579539e307;

/// e_exp.c, with V8's exp(1) = E
pub fn exp(mut x: f64) -> f64 {
    let mut hx = high(x);
    let xsb = (hx as u32 >> 31) as usize; /* sign bit of x */
    hx &= 0x7fffffff; /* high word of |x| */
    let hi;
    let lo;
    let k: i32;

    /* filter out non-finite argument */
    if hx >= 0x40862e42 {
        /* if |x|>=709.78... */
        if hx >= 0x7ff00000 {
            if ((hx & 0xfffff) as u32 | low(x)) != 0 {
                return x + x; /* NaN */
            }
            return if xsb == 0 { x } else { 0.0 }; /* exp(+-inf)={inf,0} */
        }
        if x > O_THRESHOLD {
            return HUGE * HUGE; /* overflow */
        }
        if x < U_THRESHOLD {
            return TWOM1000 * TWOM1000; /* underflow */
        }
    }

    /* argument reduction */
    if hx > 0x3fd62e42 {
        /* if  |x| > 0.5 ln2 */
        if hx < 0x3ff0a2b2 {
            /* and |x| < 1.5 ln2 */
            if x == 1.0 {
                return E; // V8: the reduction below gets the last bit of e wrong
            }
            hi = x - LN2HI[xsb];
            lo = LN2LO[xsb];
            k = 1 - xsb as i32 - xsb as i32;
        } else {
            k = (INVLN2 * x + HALF[xsb]) as i32;
            let t = k as f64;
            hi = x - t * LN2HI[0]; /* t*ln2HI is exact here */
            lo = t * LN2LO[0];
        }
        x = hi - lo;
    } else if hx < 0x3e300000 {
        /* when |x|<2**-28 */
        return 1.0 + x;
    } else {
        k = 0;
        hi = 0.0;
        lo = 0.0;
    }

    /* x is now in primary range */
    let t = x * x;
    let twopk = if k >= -1021 {
        words(0x3ff00000i32.wrapping_add(k << 20), 0)
    } else {
        words(0x3ff00000 + ((k + 1000) << 20), 0)
    };
    let c = x - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))));
    if k == 0 {
        return 1.0 - ((x * c) / (c - 2.0) - x);
    }
    let y = 1.0 - ((lo - (x * c) / (2.0 - c)) - hi);
    if k >= -1021 {
        if k == 1024 {
            return y * 2.0 * TWO1023;
        }
        return y * twopk;
    }
    y * twopk * TWOM1000
}

const Q1: f64 = -3.33333333333331316428e-02;
const Q2: f64 = 1.58730158725481460165e-03;
const Q3: f64 = -7.93650757867487942473e-05;
const Q4: f64 = 4.00821782732936239552e-06;
const Q5: f64 = -2.01099218183624371326e-07;

/// s_expm1.c, as V8 has it
pub fn expm1(mut x: f64) -> f64 {
    let mut hx = high(x);
    let xsb = hx & SIGN; /* sign bit of x */
    hx &= 0x7fffffff; /* high word of |x| */
    let k: i32;
    let mut c = 0.0;

    /* filter out huge and non-finite argument */
    if hx >= 0x4043687a {
        /* if |x|>=56*ln2 */
        if hx >= 0x40862e42 {
            /* if |x|>=709.78... */
            if hx >= 0x7ff00000 {
                if ((hx & 0xfffff) as u32 | low(x)) != 0 {
                    return x + x; /* NaN */
                }
                return if xsb == 0 { x } else { -1.0 }; /* exp(+-inf)={inf,-1} */
            }
            if x > O_THRESHOLD {
                return HUGE * HUGE; /* overflow */
            }
        }
        if xsb != 0 && x + TINY < 0.0 {
            /* x < -56*ln2, return -1.0 with inexact */
            return TINY - 1.0;
        }
    }

    /* argument reduction */
    if hx > 0x3fd62e42 {
        /* if  |x| > 0.5 ln2 */
        let hi;
        let lo;
        if hx < 0x3ff0a2b2 {
            /* and |x| < 1.5 ln2 */
            if xsb == 0 {
                hi = x - LN2_HI;
                lo = LN2_LO;
                k = 1;
            } else {
                hi = x + LN2_HI;
                lo = -LN2_LO;
                k = -1;
            }
        } else {
            k = (INVLN2 * x + if xsb == 0 { 0.5 } else { -0.5 }) as i32;
            let t = k as f64;
            hi = x - t * LN2_HI; /* t*ln2_hi is exact here */
            lo = t * LN2_LO;
        }
        x = hi - lo;
        c = (hi - x) - lo;
    } else if hx < 0x3c900000 {
        /* when |x|<2**-54, return x */
        let t = HUGE + x; /* return x with inexact flags when x!=0 */
        return x - (t - (HUGE + x));
    } else {
        k = 0;
    }

    /* x is now in primary range */
    let hfx = 0.5 * x;
    let hxs = x * hfx;
    let r1 = 1.0 + hxs * (Q1 + hxs * (Q2 + hxs * (Q3 + hxs * (Q4 + hxs * Q5))));
    let mut t = 3.0 - r1 * hfx;
    let mut e = hxs * ((r1 - t) / (6.0 - x * t));
    if k == 0 {
        return x - (x * e - hxs); /* c is 0 */
    }
    let twopk = words(0x3ff00000i32.wrapping_add(k << 20), 0); /* 2^k */
    e = x * (e - c) - c;
    e -= hxs;
    if k == -1 {
        return 0.5 * (x - e) - 0.5;
    }
    if k == 1 {
        if x < -0.25 {
            return -2.0 * (e - (x + 0.5));
        }
        return 1.0 + 2.0 * (x - e);
    }
    let mut y;
    if k <= -2 || k > 56 {
        /* suffice to return exp(x)-1 */
        y = 1.0 - (e - x);
        if k == 1024 {
            y = y * 2.0 * 8.98846567431158e+307; // V8's literal; it is exactly 2**1023
        } else {
            y = y * twopk;
        }
        return y - 1.0;
    }
    if k < 20 {
        t = words(0x3ff00000 - (0x200000 >> k), 0); /* t=1-2^-k */
        y = t - (e - x);
        y = y * twopk;
    } else {
        t = words((0x3ff - k) << 20, 0); /* 2^-k */
        y = x - (e + t);
        y += 1.0;
        y = y * twopk;
    }
    y
}

const LG1: f64 = 6.666666666666735130e-01;
const LG2: f64 = 3.999999999940941908e-01;
const LG3: f64 = 2.857142874366239149e-01;
const LG4: f64 = 2.222219843214978396e-01;
const LG5: f64 = 1.818357216161805012e-01;
const LG6: f64 = 1.531383769920937332e-01;
const LG7: f64 = 1.479819860511658591e-01;

/// e_log.c: the natural logarithm.
pub fn ln(mut x: f64) -> f64 {
    let mut hx = high(x);
    let lx = low(x);
    let mut k = 0;
    if hx < 0x00100000 {
        /* x < 2**-1022  */
        if ((hx & 0x7fffffff) as u32 | lx) == 0 {
            return f64::NEG_INFINITY; /* log(+-0)=-inf */
        }
        if hx < 0 {
            return f64::NAN; /* log(-#) = NaN */
        }
        k -= 54;
        x *= TWO54; /* subnormal number, scale up x */
        hx = high(x);
    }
    if hx >= 0x7ff00000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000fffff;
    let i = (hx + 0x95f64) & 0x100000;
    x = with_high(x, hx | (i ^ 0x3ff00000)); /* normalize x or x/2 */
    k += i >> 20;
    let f = x - 1.0;
    if (0x000fffff & (2 + hx)) < 3 {
        /* -2**-20 <= f < 2**-20 */
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            let dk = k as f64;
            return dk * LN2_HI + dk * LN2_LO;
        }
        let r = f * f * (0.5 - 0.33333333333333333 * f);
        if k == 0 {
            return f - r;
        }
        let dk = k as f64;
        return dk * LN2_HI - ((r - dk * LN2_LO) - f);
    }
    let s = f / (2.0 + f);
    let dk = k as f64;
    let z = s * s;
    let mut i = hx - 0x6147a;
    let w = z * z;
    let j = 0x6b851 - hx;
    let t1 = w * (LG2 + w * (LG4 + w * LG6));
    let t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    i |= j;
    let r = t2 + t1;
    if i > 0 {
        let hfsq = 0.5 * f * f;
        if k == 0 {
            return f - (hfsq - s * (hfsq + r));
        }
        return dk * LN2_HI - ((hfsq - (s * (hfsq + r) + dk * LN2_LO)) - f);
    }
    if k == 0 {
        return f - s * (f - r);
    }
    dk * LN2_HI - ((s * (f - r) - dk * LN2_LO) - f)
}

/// s_log1p.c (Lp1..Lp7 are Lg1..Lg7): ln(1 + x).
pub fn ln_1p(x: f64) -> f64 {
    let hx = high(x);
    let ax = hx & 0x7fffffff;
    let mut k = 1;
    let mut f = 0.0;
    let mut hu = 0;
    let mut c = 0.0;
    if hx < 0x3fda827a {
        /* 1+x < sqrt(2)+ */
        if ax >= 0x3ff00000 {
            /* x <= -1.0 */
            if x == -1.0 {
                return f64::NEG_INFINITY; /* log1p(-1)=-inf */
            }
            return f64::NAN; /* log1p(x<-1)=NaN */
        }
        if ax < 0x3e200000 {
            /* |x| < 2**-29 */
            if ax < 0x3c900000 {
                return x; /* |x| < 2**-54 */
            }
            return x - x * x * 0.5;
        }
        if hx > 0 || hx <= 0xbfd2bec4u32 as i32 {
            /* sqrt(2)/2- <= 1+x < sqrt(2)+ */
            k = 0;
            f = x;
            hu = 1;
        }
    }
    if hx >= 0x7ff00000 {
        return x + x;
    }
    if k != 0 {
        let mut u;
        if hx < 0x43400000 {
            u = 1.0 + x;
            hu = high(u);
            k = (hu >> 20) - 1023;
            c = if k > 0 { 1.0 - (u - x) } else { x - (u - 1.0) }; /* correction term */
            c /= u;
        } else {
            u = x;
            hu = high(u);
            k = (hu >> 20) - 1023;
            c = 0.0;
        }
        hu &= 0x000fffff;
        if hu < 0x6a09e {
            /* u ~< sqrt(2) */
            u = with_high(u, hu | 0x3ff00000); /* normalize u */
        } else {
            k += 1;
            u = with_high(u, hu | 0x3fe00000); /* normalize u/2 */
            hu = (0x00100000 - hu) >> 2;
        }
        f = u - 1.0;
    }
    let hfsq = 0.5 * f * f;
    let dk = k as f64;
    if hu == 0 {
        /* |f| < 2**-20 */
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            c += dk * LN2_LO;
            return dk * LN2_HI + c;
        }
        let r = hfsq * (1.0 - 0.66666666666666666 * f);
        if k == 0 {
            return f - r;
        }
        return dk * LN2_HI - ((r - (dk * LN2_LO + c)) - f);
    }
    let s = f / (2.0 + f);
    let z = s * s;
    let r = z * (LG1 + z * (LG2 + z * (LG3 + z * (LG4 + z * (LG5 + z * (LG6 + z * LG7))))));
    if k == 0 {
        return f - (hfsq - s * (hfsq + r));
    }
    dk * LN2_HI - ((hfsq - (s * (hfsq + r) + (dk * LN2_LO + c))) - f)
}

/// k_log1p (FreeBSD k_log.h, as V8 has it): log(1+f) - f + f*f/2 for 1+f in ~[sqrt(2)/2, sqrt(2)].
fn k_log1p(f: f64) -> f64 {
    let s = f / (2.0 + f);
    let z = s * s;
    let w = z * z;
    let t1 = w * (LG2 + w * (LG4 + w * LG6));
    let t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    let r = t2 + t1;
    let hfsq = 0.5 * f * f;
    s * (hfsq + r)
}

const IVLN2HI: f64 = 1.44269504072144627571e+00;
const IVLN2LO: f64 = 1.67517131648865118353e-10;

/// e_log2.c (FreeBSD msun, as V8 has it)
pub fn log2(mut x: f64) -> f64 {
    let mut hx = high(x);
    let lx = low(x);
    let mut k = 0;
    if hx < 0x00100000 {
        if ((hx & 0x7fffffff) as u32 | lx) == 0 {
            return f64::NEG_INFINITY;
        }
        if hx < 0 {
            return f64::NAN;
        }
        k -= 54;
        x *= TWO54;
        hx = high(x);
    }
    if hx >= 0x7ff00000 {
        return x + x;
    }
    if hx == 0x3ff00000 && lx == 0 {
        return 0.0; /* log(1) = +0 */
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000fffff;
    let i = (hx + 0x95f64) & 0x100000;
    x = with_high(x, hx | (i ^ 0x3ff00000)); /* normalize x or x/2 */
    k += i >> 20;
    let y = k as f64;
    let f = x - 1.0;
    let hfsq = 0.5 * f * f;
    let r = k_log1p(f);
    /* f-hfsq in extra precision: hi has its low word cleared (Dekker) */
    let hi = with_low(f - hfsq, 0);
    let lo = (f - hi) - hfsq + r;
    let mut val_hi = hi * IVLN2HI;
    let mut val_lo = (lo + hi) * IVLN2LO + lo * IVLN2HI;
    /* spadd(val_hi, val_lo, y), except for not using double_t: */
    let w = y + val_hi;
    val_lo += (y - w) + val_hi;
    val_hi = w;
    val_lo + val_hi
}

const IVLN10: f64 = 4.34294481903251816668e-01;
const LOG10_2HI: f64 = 3.01029995663611771306e-01;
const LOG10_2LO: f64 = 3.69423907715893078616e-13;

/// e_log10.c, as V8 has it
pub fn log10(mut x: f64) -> f64 {
    let mut hx = high(x);
    let mut lx = low(x);
    let mut k = 0;
    if hx < 0x00100000 {
        if ((hx & 0x7fffffff) as u32 | lx) == 0 {
            return f64::NEG_INFINITY;
        }
        if hx < 0 {
            return f64::NAN;
        }
        k -= 54;
        x *= TWO54;
        hx = high(x);
        lx = low(x);
    }
    if hx >= 0x7ff00000 {
        return x + x;
    }
    if hx == 0x3ff00000 && lx == 0 {
        return 0.0;
    }
    k += (hx >> 20) - 1023;
    let i = ((k as u32) >> 31) as i32;
    hx = (hx & 0x000fffff) | ((0x3ff - i) << 20);
    let y = (k + i) as f64;
    x = words(hx, lx);
    let z = y * LOG10_2LO + IVLN10 * ln(x);
    z + y * LOG10_2HI
}

const BP: [f64; 2] = [1.0, 1.5];
const DP_H: [f64; 2] = [0.0, 5.84962487220764160156e-01];
const DP_L: [f64; 2] = [0.0, 1.35003920212974897128e-08];
const L1: f64 = 5.99999999999994648725e-01;
const L2: f64 = 4.28571428578550184252e-01;
const L3: f64 = 3.33333329818377432918e-01;
const L4: f64 = 2.72728123808534006489e-01;
const L5: f64 = 2.30660745775561754067e-01;
const L6: f64 = 2.06975017800338417784e-01;
const LG2_: f64 = 6.93147180559945286227e-01;
const LG2_H: f64 = 6.93147182464599609375e-01;
const LG2_L: f64 = -1.90465429995776804525e-09;
const OVT: f64 = 8.0085662595372944372e-0017;
const CP: f64 = 9.61796693925975554329e-01;
const CP_H: f64 = 9.61796700954437255859e-01;
const CP_L: f64 = -7.02846165095275826516e-09;
const IVLN2: f64 = 1.44269504088896338700e+00;
const IVLN2_H: f64 = 1.44269502162933349609e+00;
const IVLN2_L: f64 = 1.92596299112661746887e-08;

/// e_pow.c, as V8 has it: x ** y, with NaN for (+-1) ** (+-inf) as ECMAScript requires.
pub fn pow(x: f64, y: f64) -> f64 {
    let hx = high(x);
    let lx = low(x);
    let hy = high(y);
    let ly = low(y);
    let mut ix = hx & 0x7fffffff;
    let iy = hy & 0x7fffffff;

    /* y==zero: x**0 = 1 */
    if (iy as u32 | ly) == 0 {
        return 1.0;
    }

    /* +-NaN return x+y */
    if ix > 0x7ff00000
        || (ix == 0x7ff00000 && lx != 0)
        || iy > 0x7ff00000
        || (iy == 0x7ff00000 && ly != 0)
    {
        return x + y;
    }

    /* yisint = 0: y is not an integer, 1: an odd int, 2: an even int (only when x < 0) */
    let mut yisint = 0;
    if hx < 0 {
        if iy >= 0x43400000 {
            yisint = 2; /* even integer y */
        } else if iy >= 0x3ff00000 {
            let k = (iy >> 20) - 0x3ff; /* exponent */
            if k > 20 {
                let j = ly >> (52 - k);
                if (j << (52 - k)) == ly {
                    yisint = 2 - (j & 1) as i32;
                }
            } else if ly == 0 {
                let j = iy >> (20 - k);
                if (j << (20 - k)) == iy {
                    yisint = 2 - (j & 1);
                }
            }
        }
    }

    /* special value of y */
    if ly == 0 {
        if iy == 0x7ff00000 {
            /* y is +-inf */
            if ((ix - 0x3ff00000) as u32 | lx) == 0 {
                return y - y; /* inf**+-1 is NaN */
            }
            if ix >= 0x3ff00000 {
                return if hy >= 0 { y } else { 0.0 }; /* (|x|>1)**+-inf = inf,0 */
            }
            return if hy < 0 { -y } else { 0.0 }; /* (|x|<1)**-,+inf = inf,0 */
        }
        if iy == 0x3ff00000 {
            return if hy < 0 { 1.0 / x } else { x }; /* y is  +-1 */
        }
        if hy == 0x40000000 {
            return x * x; /* y is  2 */
        }
        if hy == 0x3fe00000 && hx >= 0 {
            return x.sqrt(); /* y is  0.5, x >= +0 */
        }
    }

    let mut ax = x.abs();
    /* special value of x */
    if lx == 0 && (ix == 0x7ff00000 || ix == 0 || ix == 0x3ff00000) {
        let mut z = ax; /* x is +-0,+-inf,+-1 */
        if hy < 0 {
            z = 1.0 / z; /* z = (1/|x|) */
        }
        if hx < 0 {
            if ((ix - 0x3ff00000) | yisint) == 0 {
                z = f64::NAN; /* (-1)**non-int is NaN */
            } else if yisint == 1 {
                z = -z; /* (x<0)**odd = -(|x|**odd) */
            }
        }
        return z;
    }

    let mut n = (hx >> 31) + 1;

    /* (x<0)**(non-int) is NaN */
    if (n | yisint) == 0 {
        return f64::NAN;
    }

    let mut s = 1.0; /* s (sign of result -ve**odd) = -1 else = 1 */
    if (n | (yisint - 1)) == 0 {
        s = -1.0; /* (-ve)**(odd int) */
    }

    let t1;
    let t2;
    /* |y| is huge */
    if iy > 0x41e00000 {
        /* if |y| > 2**31 */
        if iy > 0x43f00000 {
            /* if |y| > 2**64, must o/uflow */
            if ix <= 0x3fefffff {
                return if hy < 0 { HUGE * HUGE } else { TINY * TINY };
            }
            if ix >= 0x3ff00000 {
                return if hy > 0 { HUGE * HUGE } else { TINY * TINY };
            }
        }
        /* over/underflow if x is not close to one */
        if ix < 0x3fefffff {
            return if hy < 0 { s * HUGE * HUGE } else { s * TINY * TINY };
        }
        if ix > 0x3ff00000 {
            return if hy > 0 { s * HUGE * HUGE } else { s * TINY * TINY };
        }
        /* now |1-x| is tiny <= 2**-20, suffice to compute log(x) by x-x^2/2+x^3/3-x^4/4 */
        let t = ax - 1.0; /* t has 20 trailing zeros */
        let w = (t * t) * (0.5 - t * (0.3333333333333333333333 - t * 0.25));
        let u = IVLN2_H * t; /* ivln2_h has 21 sig. bits */
        let v = t * IVLN2_L - w * IVLN2;
        t1 = with_low(u + v, 0);
        t2 = v - (t1 - u);
    } else {
        n = 0;
        /* take care subnormal number */
        if ix < 0x00100000 {
            ax *= TWO53;
            n -= 53;
            ix = high(ax);
        }
        n += (ix >> 20) - 0x3ff;
        let j = ix & 0x000fffff;
        /* determine interval */
        ix = j | 0x3ff00000; /* normalize ix */
        let k: usize;
        if j <= 0x3988e {
            k = 0; /* |x|<sqrt(3/2) */
        } else if j < 0xbb67a {
            k = 1; /* |x|<sqrt(3)   */
        } else {
            k = 0;
            n += 1;
            ix -= 0x00100000;
        }
        ax = with_high(ax, ix);

        /* compute ss = s_h+s_l = (x-1)/(x+1) or (x-1.5)/(x+1.5) */
        let mut u = ax - BP[k]; /* bp[0]=1.0, bp[1]=1.5 */
        let mut v = 1.0 / (ax + BP[k]);
        let ss = u * v;
        let s_h = with_low(ss, 0);
        /* t_h=ax+bp[k] High */
        let mut t_h = words(((ix >> 1) | 0x20000000) + 0x00080000 + ((k as i32) << 18), 0);
        let mut t_l = ax - (t_h - BP[k]);
        let s_l = v * ((u - s_h * t_h) - s_h * t_l);
        /* compute log(ax) */
        let mut s2 = ss * ss;
        let mut r = s2 * s2 * (L1 + s2 * (L2 + s2 * (L3 + s2 * (L4 + s2 * (L5 + s2 * L6)))));
        r += s_l * (s_h + ss);
        s2 = s_h * s_h;
        t_h = with_low(3.0 + s2 + r, 0);
        t_l = r - ((t_h - 3.0) - s2);
        /* u+v = ss*(1+...) */
        u = s_h * t_h;
        v = s_l * t_h + t_l * ss;
        /* 2/(3log2)*(ss+...) */
        let p_h = with_low(u + v, 0);
        let p_l = v - (p_h - u);
        let z_h = CP_H * p_h; /* cp_h+cp_l = 2/(3*log2) */
        let z_l = CP_L * p_h + p_l * CP + DP_L[k];
        /* log2(ax) = (ss+..)*2/(3*log2) = n + dp_h + z_h + z_l */
        let t = n as f64;
        t1 = with_low(((z_h + z_l) + DP_H[k]) + t, 0);
        t2 = z_l - (((t1 - t) - DP_H[k]) - z_h);
    }

    /* split up y into y1+y2 and compute (y1+y2)*(t1+t2) */
    let y1 = with_low(y, 0);
    let p_l = (y - y1) * t1 + y * t2;
    let mut p_h = y1 * t1;
    let mut z = p_l + p_h;
    let mut j = high(z);
    let mut i = low(z) as i32;
    if j >= 0x40900000 {
        /* z >= 1024 */
        if ((j - 0x40900000) | i) != 0 {
            return s * HUGE * HUGE; /* if z > 1024: overflow */
        }
        if p_l + OVT > z - p_h {
            return s * HUGE * HUGE; /* overflow */
        }
    } else if (j & 0x7fffffff) >= 0x4090cc00 {
        /* z <= -1075 */
        if (j.wrapping_sub(0xc090cc00u32 as i32) | i) != 0 {
            return s * TINY * TINY; /* z < -1075: underflow */
        }
        if p_l <= z - p_h {
            return s * TINY * TINY; /* underflow */
        }
    }
    /* compute 2**(p_h+p_l) */
    i = j & 0x7fffffff;
    let mut k = (i >> 20) - 0x3ff;
    n = 0;
    if i > 0x3fe00000 {
        /* if |z| > 0.5, set n = [z+0.5] */
        n = j.wrapping_add(0x00100000 >> (k + 1));
        k = ((n & 0x7fffffff) >> 20) - 0x3ff; /* new k for n */
        let t = words(n & !(0x000fffff >> k), 0);
        n = ((n & 0x000fffff) | 0x00100000) >> (20 - k);
        if j < 0 {
            n = -n;
        }
        p_h -= t;
    }
    let mut t = with_low(p_l + p_h, 0);
    let u = t * LG2_H;
    let v = (p_l - (t - p_h)) * LG2_ + t * LG2_L;
    z = u + v;
    let w = v - (z - u);
    t = z * z;
    let t1 = z - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))));
    let r = (z * t1) / ((t1 - 2.0) - (w + z * w));
    z = 1.0 - (r - z);
    j = high(z).wrapping_add(n << 20);
    if (j >> 20) <= 0 {
        z = scalbn(z, n); /* subnormal output */
    } else {
        z = with_high(z, high(z).wrapping_add(n << 20));
    }
    s * z
}

const KCOSH_OVERFLOW: f64 = 710.4758600739439;

/// V8's cosh (ES6 draft 09-27-13, 20.2.2.12)
pub fn cosh(x: f64) -> f64 {
    let ix = high(x) & 0x7fffffff;
    // |x| in [0,0.5*log2], return 1+expm1(|x|)^2/(2*exp(|x|))
    if ix < 0x3fd62e43 {
        let t = expm1(x.abs());
        let w = 1.0 + t;
        // For |x| < 2^-55, cosh(x) = 1
        if ix < 0x3c800000 {
            return w;
        }
        return 1.0 + (t * t) / (w + w);
    }
    // |x| in [0.5*log2, 22], return (exp(|x|)+1/exp(|x|)/2
    if ix < 0x40360000 {
        let t = exp(x.abs());
        return 0.5 * t + 0.5 / t;
    }
    // |x| in [22, log(maxdouble)], return half*exp(|x|)
    if ix < 0x40862e42 {
        return 0.5 * exp(x.abs());
    }
    // |x| in [log(maxdouble), overflowthreshold]
    if x.abs() <= KCOSH_OVERFLOW {
        let w = exp(0.5 * x.abs());
        let t = 0.5 * w;
        return t * w;
    }
    /* x is INF or NaN */
    if ix >= 0x7ff00000 {
        return x * x;
    }
    // |x| > overflowthreshold.
    HUGE * HUGE
}

const KSINH_OVERFLOW: f64 = 710.4758600739439;
const TWO_M28: f64 = 3.725290298461914e-9; // 2^-28, empty lower half
const LOG_MAXD: f64 = 709.7822265625; // 0x40862E42 00000000, empty lower half
const SHUGE: f64 = 1.0e307;

/// V8's sinh (ES6 draft 09-27-13, 20.2.2.30)
pub fn sinh(x: f64) -> f64 {
    let h = if x < 0.0 { -0.5 } else { 0.5 };
    // |x| in [0, 22]. return sign(x)*0.5*(E+E/(E+1))
    let ax = x.abs();
    if ax < 22.0 {
        // For |x| < 2^-28, sinh(x) = x
        if ax < TWO_M28 {
            return x;
        }
        let t = expm1(ax);
        if ax < 1.0 {
            return h * (2.0 * t - t * t / (t + 1.0));
        }
        return h * (t + t / (t + 1.0));
    }
    // |x| in [22, log(maxdouble)], return 0.5 * exp(|x|)
    if ax < LOG_MAXD {
        return h * exp(ax);
    }
    // |x| in [log(maxdouble), overflowthreshold]
    if ax <= KSINH_OVERFLOW {
        let w = exp(0.5 * ax);
        let t = h * w;
        return t * w;
    }
    // |x| > overflowthreshold or is NaN. Return Infinity of the appropriate sign or NaN.
    x * SHUGE
}

/// s_tanh.c, as V8 has it
pub fn tanh(x: f64) -> f64 {
    let jx = high(x);
    let ix = jx & 0x7fffffff;
    /* x is INF or NaN */
    if ix >= 0x7ff00000 {
        if jx >= 0 {
            return 1.0 / x + 1.0; /* tanh(+-inf)=+-1 */
        }
        return 1.0 / x - 1.0; /* tanh(NaN) = NaN */
    }
    let z;
    if ix < 0x40360000 {
        /* |x|<22 */
        if ix < 0x3e300000 {
            return x; /* |x|<2**-28 */
        }
        if ix >= 0x3ff00000 {
            /* |x|>=1  */
            let t = expm1(2.0 * x.abs());
            z = 1.0 - 2.0 / (t + 2.0);
        } else {
            let t = expm1(-2.0 * x.abs());
            z = -t / (t + 2.0);
        }
    } else {
        /* |x| >= 22, return +-1 */
        z = 1.0 - TINY; /* raise inexact flag */
    }
    if jx >= 0 { z } else { -z }
}

const LN2: f64 = 6.93147180559945286227e-01;

/// s_asinh.c
pub fn asinh(x: f64) -> f64 {
    let hx = high(x);
    let ix = hx & 0x7fffffff;
    if ix >= 0x7ff00000 {
        return x + x; /* x is inf or NaN */
    }
    if ix < 0x3e300000 {
        return x; /* |x|<2**-28 */
    }
    let w;
    if ix > 0x41b00000 {
        /* |x| > 2**28 */
        w = ln(x.abs()) + LN2;
    } else if ix > 0x40000000 {
        /* 2**28 > |x| > 2.0 */
        let t = x.abs();
        w = ln(2.0 * t + 1.0 / ((x * x + 1.0).sqrt() + t));
    } else {
        /* 2.0 > |x| > 2**-28 */
        let t = x * x;
        w = ln_1p(x.abs() + t / (1.0 + (1.0 + t).sqrt()));
    }
    if hx > 0 { w } else { -w }
}

/// e_acosh.c
pub fn acosh(x: f64) -> f64 {
    let hx = high(x);
    let lx = low(x);
    if hx < 0x3ff00000 {
        return f64::NAN; /* x < 1 */
    }
    if hx >= 0x41b00000 {
        /* x > 2**28 */
        if hx >= 0x7ff00000 {
            return x + x; /* x is inf of NaN */
        }
        return ln(x) + LN2; /* acosh(huge)=log(2x) */
    }
    if ((hx - 0x3ff00000) as u32 | lx) == 0 {
        return 0.0; /* acosh(1) = 0 */
    }
    if hx > 0x40000000 {
        /* 2**28 > x > 2 */
        let t = x * x;
        return ln(2.0 * x - 1.0 / (x + (t - 1.0).sqrt()));
    }
    let t = x - 1.0; /* 1<x<2 */
    ln_1p(t + (2.0 * t + t * t).sqrt())
}

/// e_atanh.c, as V8 has it
pub fn atanh(mut x: f64) -> f64 {
    let hx = high(x);
    let lx = low(x);
    let ix = hx & 0x7fffffff;
    if (ix as u32 | ((lx | lx.wrapping_neg()) >> 31)) > 0x3ff00000 {
        return f64::NAN; /* |x|>1 */
    }
    if ix == 0x3ff00000 {
        return if x > 0.0 { f64::INFINITY } else { f64::NEG_INFINITY };
    }
    if ix < 0x3e300000 {
        return x; /* x<2**-28 */
    }
    x = with_high(x, ix); /* |x| */
    let mut t;
    if ix < 0x3fe00000 {
        /* x < 0.5 */
        t = x + x;
        t = 0.5 * ln_1p(t + t * x / (1.0 - x));
    } else {
        t = 0.5 * ln_1p((x + x) / (1.0 - x));
    }
    if hx >= 0 { t } else { -t }
}

const B1: i32 = 715094163; /* B1 = (1023-1023/3-0.03306235651)*2**20 */
const B2: i32 = 696219795; /* B2 = (1023-1023/3-54/3-0.03306235651)*2**20 */
/* |1/cbrt(x) - p(x)| < 2**-23.5 (~[-7.93e-8, 7.929e-8]). */
const CP0: f64 = 1.87595182427177009643;
const CP1: f64 = -1.88497979543377169875;
const CP2: f64 = 1.621429720105354466140;
const CP3: f64 = -0.758397934778766047437;
const CP4: f64 = 0.145996192886612446982;

/// s_cbrt.c (FreeBSD msun, as V8 has it)
pub fn cbrt(x: f64) -> f64 {
    let mut hx = high(x);
    let lx = low(x);
    let sign = hx & SIGN;
    hx ^= sign;
    if hx >= 0x7ff00000 {
        return x + x; /* cbrt(NaN,INF) is itself */
    }

    /* Rough cbrt to 5 bits: integer division of the exponent's bits by 3 (see the C source). */
    let mut t;
    if hx < 0x00100000 {
        /* zero or subnormal? */
        if (hx as u32 | lx) == 0 {
            return x; /* cbrt(0) is itself */
        }
        t = words(0x43500000, 0) * x; /* t = 2**54 * x */
        t = words(sign | ((high(t) & 0x7fffffff) / 3 + B2), 0);
    } else {
        t = words(sign | (hx / 3 + B1), 0);
    }

    /* New cbrt to 23 bits: cbrt(x) = t*cbrt(x/t**3) ~= t*P(t**3/x) */
    let mut r = (t * t) * (t / x);
    t = t * ((CP0 + r * (CP1 + r * CP2)) + ((r * r) * r) * (CP3 + r * CP4));

    /* Round t away from zero to 23 bits */
    t = f64::from_bits(t.to_bits().wrapping_add(0x80000000) & 0xffffffffc0000000);

    /* one step Newton iteration to 53 bits with error < 0.667 ulps */
    let s = t * t; /* t*t is exact */
    r = x / s; /* error <= 0.5 ulps; |r| < |t| */
    let w = t + t; /* t+t is exact */
    r = (r - t) / (w + r); /* r-t is exact; w+r ~= 3*t */
    t + t * r /* error <= 0.5 + 0.5/3 + epsilon */
}

/// hypot of any number of values: max = the largest magnitude; the squares of each value / max
/// summed with Kahan compensation; sqrt(sum) * max. Infinity wins over NaN and no values give
/// +0. This is V8's Math.hypot algorithm (not an fdlibm function), matched bit for bit.
pub fn hypot(values: &[f64]) -> f64 {
    let mut max: f64 = 0.0;
    let mut nan = false;
    for &v in values {
        if v.is_nan() {
            nan = true;
        } else if v.abs() > max {
            max = v.abs();
        }
    }
    if max == f64::INFINITY {
        return f64::INFINITY;
    }
    if nan {
        return f64::NAN;
    }
    if max == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut compensation = 0.0;
    for &v in values {
        let n = if v.is_nan() { 0.0 } else { v.abs() } / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}

/// hypot(a, b) without a slice.
pub fn hypot2(a: f64, b: f64) -> f64 {
    hypot(&[a, b])
}

#[cfg(test)]
mod tests;
