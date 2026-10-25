//! The math check: an input
//! generator (integer steps and exact double arithmetic only, so every target makes the same
//! inputs) and the same 64-bit result hash. `GOLDEN` holds the pinned hashes
//! for the first 50,000 inputs per function, which are x64 V8's own Math results. Matching them
//! proves this port and the generator port both, and running `combined_hash` on every build
//! target proves the targets agree.

use crate as m;

/// The pinned hashes for `run(name, 50_000)`.
pub const GOLDEN: [(&str, &str); 23] = [
    ("exp", "e1e0a982cf762a5c"),
    ("expm1", "a85badaf3b7df14d"),
    ("log", "ff7abe3ba61b89d7"),
    ("log1p", "895101e6747eefad"),
    ("log2", "e11225de62934982"),
    ("log10", "15194145cfd9e93b"),
    ("sin", "3c45e7648348b584"),
    ("cos", "557173ea8dd610fc"),
    ("tan", "6df5c1d0142b2f7e"),
    ("asin", "9dfbb0785ec3a7a7"),
    ("acos", "18d174eff65a8690"),
    ("atan", "b46ce9ff701f9caa"),
    ("sinh", "040b348a08d92464"),
    ("cosh", "fe2f02777361ee4a"),
    ("tanh", "f5dd69a530ee9661"),
    ("asinh", "54ac4c5e6e2d3a3d"),
    ("acosh", "b1440bb9bb767598"),
    ("atanh", "4b5ed431acd1b52e"),
    ("cbrt", "e362a07d3083af7c"),
    ("pow", "3d9506cb9c621d19"),
    ("atan2", "1aa2ea9992e77dcc"),
    ("hypot", "a5ab1514c918cb19"),
    ("hypot3", "601b680a6c98a8e8"),
];

/// The check names, in the reference order.
pub fn names() -> impl Iterator<Item = &'static str> {
    GOLDEN.iter().map(|(n, _)| *n)
}

fn bits(hi: u32, lo: u32) -> f64 {
    f64::from_bits(((hi as u64) << 32) | lo as u64)
}

/// x moved k representable doubles away from zero (k < 0: towards it), staying finite.
fn ulps(x: f64, k: i64) -> f64 {
    let b = x.to_bits();
    let sign = (b >> 32) as u32 & 0x80000000;
    let mut hi = ((b >> 32) as u32 & 0x7fffffff) as i64;
    let mut lo = (b as u32) as i64 + k;
    if lo < 0 {
        lo += 1 << 32;
        hi -= 1;
    } else if lo >= 1 << 32 {
        lo -= 1 << 32;
        hi += 1;
    }
    if !(0..0x7ff00000).contains(&hi) {
        return x;
    }
    bits(hi as u32 | sign, lo as u32)
}

/// 2^e for an integer e in [-1074, 1023], built from bits.
fn pow2(e: i32) -> f64 {
    if e >= -1022 {
        return bits(((e + 1023) as u32) << 20, 0);
    }
    let k = e + 1074; // subnormal: bit k of the fraction
    if k >= 32 { bits(1 << (k - 32), 0) } else { bits(0, 1 << k) }
}

const TWO53: f64 = 9007199254740992.0;
const PIO2_HI: f64 = 1.5707963267948966;
const PIO2_LO: f64 = 6.123233995736766e-17;
const LN2: f64 = 0.6931471805599453;
const MAX: f64 = f64::MAX;
const MIN_NORMAL: f64 = 2.2250738585072014e-308;
const MAX_SUBNORMAL: f64 = 2.225073858507201e-308;
const INF: f64 = f64::INFINITY;
const NAN: f64 = f64::NAN;

/// xorshift128 over 32-bit words, as the JS generator.
struct Gen {
    s: [u32; 4],
}

impl Gen {
    fn new(seed: u32) -> Gen {
        let a = if seed == 0 { 1 } else { seed };
        let mut g = Gen { s: [a, 0x9e3779b9 ^ seed, 0x243f6a88, 0xb7e15162] };
        for _ in 0..64 {
            g.next();
        }
        g
    }

    fn next(&mut self) -> u32 {
        let [a, b, c, d] = self.s;
        let t = a ^ (a << 11);
        let d2 = d ^ (d >> 19) ^ (t ^ (t >> 8));
        self.s = [b, c, d, d2];
        d2
    }

    fn u01(&mut self) -> f64 {
        let a = (self.next() >> 5) as f64;
        let b = (self.next() >> 6) as f64;
        (a * 67108864.0 + b) / TWO53
    }

    fn int(&mut self, n: f64) -> f64 {
        (self.u01() * n).floor()
    }

    fn pick(&mut self, list: &[f64]) -> f64 {
        list[self.int(list.len() as f64) as usize]
    }

    fn pick_u(&mut self, list: &[u32]) -> u32 {
        list[self.int(list.len() as f64) as usize]
    }

    fn sign(&mut self, x: f64) -> f64 {
        if self.next() & 1 != 0 { -x } else { x }
    }

    fn raw(&mut self) -> f64 {
        let hi = self.next();
        let lo = self.next();
        bits(hi, lo)
    }

    /// 2^e * (1 + random mantissa) for e in [emin, emax], subnormal below -1022.
    fn log_uniform(&mut self, emin: i32, emax: i32) -> f64 {
        let e = emin + self.int((emax - emin + 1) as f64) as i32;
        if e >= -1022 {
            let hi = (((e + 1023) as u32) << 20) | (self.next() & 0xfffff);
            let lo = self.next();
            return bits(hi, lo);
        }
        let span = pow2(e + 1074);
        (span + (self.u01() * span).floor()) * 5e-324
    }

    fn near(&mut self, x: f64, spread: f64) -> f64 {
        let k = self.int(2.0 * spread + 1.0) - spread;
        ulps(x, k as i64)
    }

    /// A double whose high word is within one of `hi_word`, any low word.
    fn at_high_word(&mut self, hi_word: u32) -> f64 {
        let hi = (hi_word as i64 + self.int(3.0) as i64 - 1) as u32;
        let lo = self.next();
        bits(hi, lo)
    }

    /// Near k * pi/2 for an integer k up to 2^k_bits.
    fn near_pio2(&mut self, k_bits: i32) -> f64 {
        let u = self.u01();
        let p = pow2(self.int((k_bits + 1) as f64) as i32);
        let k = (u * p).floor() + 1.0;
        self.near(k * PIO2_HI + k * PIO2_LO, 8.0)
    }
}

fn common_edges() -> [f64; 27] {
    [
        0.0,
        5e-324,
        1e-323,
        MAX_SUBNORMAL,
        MIN_NORMAL,
        MAX,
        INF,
        NAN,
        1.0,
        0.5,
        2.0,
        3.0,
        10.0,
        std::f64::consts::PI,
        std::f64::consts::PI / 2.0,
        std::f64::consts::PI / 4.0,
        std::f64::consts::E,
        pow2(-28),
        pow2(-27),
        pow2(-54),
        pow2(-1000),
        pow2(1000),
        1e-300,
        1e300,
        1e22,
        0.1,
        1.0 / 3.0,
    ]
}

fn edge_value(g: &mut Gen, extra: Option<&[f64]>) -> f64 {
    let common = common_edges();
    let list: &[f64] = match extra {
        Some(e) if g.int(2.0) != 0.0 => e,
        _ => &common,
    };
    let x = g.pick(list);
    if x.is_nan() {
        return NAN;
    }
    let v = g.near(x, 4.0);
    g.sign(v)
}

type Hard = fn(&mut Gen) -> f64;

struct Unary {
    edges: Option<Vec<f64>>,
    words: &'static [u32],
    emin: i32,
    emax: i32,
    range: f64,
    signed: bool,
    trig: bool,
    hard: Option<Hard>,
}

fn unary(name: &str) -> Unary {
    const TRIG_WORDS: &[u32] =
        &[0x3fe921fb, 0x4002d97c, 0x413921fb, 0x3ff921fb, 0x3e400000, 0x3fd33333, 0x3fe90000, 0x7ff00000];
    let u = |edges: &[f64], words: &'static [u32], emin, emax, range, hard: Option<Hard>| Unary {
        edges: Some(edges.to_vec()),
        words,
        emin,
        emax,
        range,
        signed: true,
        trig: false,
        hard,
    };
    let trig = |words: &'static [u32]| Unary {
        edges: None,
        words,
        emin: -1074,
        emax: 1023,
        range: 1e6,
        signed: true,
        trig: true,
        hard: None,
    };
    let unsigned = |mut c: Unary| {
        c.signed = false;
        c
    };
    let near_one_or_power: Hard = |g| {
        if g.int(2.0) != 0.0 {
            g.near(1.0, (1 << 20) as f64)
        } else {
            let e = g.int(2098.0) as i32 - 1074;
            g.near(pow2(e), 3.0)
        }
    };
    match name {
        "exp" => u(
            &[709.782712893384, -745.1332191019411, -708.3964185322641, 1.0, 0.34657359027997264, 1.0397207708399179],
            &[0x40862e42, 0x3fd62e42, 0x3ff0a2b2, 0x3e300000, 0x7ff00000, 0xc0874910],
            -60,
            10,
            750.0,
            Some(|g| {
                let x = (g.int(2100.0) - 1075.0 + 0.5) * LN2;
                let v = g.near(x, 3.0);
                g.sign(v)
            }),
        ),
        "expm1" => u(
            &[38.816242111356935, 709.782712893384, 13.862943611198906, 0.34657359027997264, 1.0397207708399179, 1.0, pow2(-54)],
            &[0x4043687a, 0x40862e42, 0x3fd62e42, 0x3ff0a2b2, 0x3c900000, 0x7ff00000],
            -80,
            10,
            60.0,
            Some(|g| {
                let x = (g.int(120.0) - 60.0 + 0.5) * LN2;
                let v = g.near(x, 3.0);
                g.sign(v)
            }),
        ),
        "log" => unsigned(u(
            &[1.0, std::f64::consts::SQRT_2, std::f64::consts::FRAC_1_SQRT_2, 2.0, 0.5, 10.0, 100.0, 1e-310],
            &[0x00100000, 0x3ff00000, 0x7ff00000, 0x3ff6a09e, 0x3fe6a09e, 0x3ff95f64],
            -1074,
            1023,
            1e6,
            Some(near_one_or_power),
        )),
        "log1p" => u(
            &[-1.0, -0.5, -0.2928932188134524, 0.41421356237309503, pow2(-29), pow2(-54), pow2(53)],
            &[0x3fda827a, 0x3ff00000, 0x3e200000, 0x3c900000, 0xbfd2bec4, 0x43400000, 0x7ff00000, 0xbff00000],
            -1074,
            1023,
            10.0,
            Some(|g| {
                if g.int(2.0) != 0.0 {
                    g.near(-1.0, (1 << 16) as f64)
                } else {
                    let v = g.log_uniform(-60, 1);
                    g.sign(v)
                }
            }),
        ),
        "log2" => unsigned(u(
            &[1.0, std::f64::consts::SQRT_2, std::f64::consts::FRAC_1_SQRT_2, 2.0, 0.5, 3.0],
            &[0x00100000, 0x3ff00000, 0x7ff00000, 0x3ff6a09e, 0x3fe6a09e],
            -1074,
            1023,
            1e6,
            Some(near_one_or_power),
        )),
        "log10" => unsigned(u(
            &[1.0, 10.0, 100.0, 1000.0, 1e22, 1e-5, 0.1, 2.0],
            &[0x00100000, 0x3ff00000, 0x7ff00000],
            -1074,
            1023,
            1e6,
            Some(|g| {
                if g.int(2.0) != 0.0 {
                    g.near(1.0, (1 << 20) as f64)
                } else {
                    let e = g.int(600.0) as i32 - 300;
                    let x: f64 = format!("1e{e}").parse().unwrap();
                    g.near(x, 3.0)
                }
            }),
        )),
        "sin" | "cos" => trig(TRIG_WORDS),
        "tan" => trig(&[0x3fe921fb, 0x4002d97c, 0x413921fb, 0x3ff921fb, 0x3e300000, 0x3fe59428, 0x7ff00000]),
        "asin" => u(
            &[1.0, 0.5, 0.975, pow2(-27), pow2(-57)],
            &[0x3ff00000, 0x3fe00000, 0x3e400000, 0x3fef3333, 0x3c600000],
            -1074,
            0,
            1.0,
            Some(|g| {
                let x = g.pick(&[1.0, 0.5, 0.975]);
                let v = g.near(x, (1 << 12) as f64);
                g.sign(v)
            }),
        ),
        "acos" => u(
            &[1.0, 0.5, pow2(-57)],
            &[0x3ff00000, 0x3fe00000, 0x3c600000],
            -1074,
            0,
            1.0,
            Some(|g| {
                let x = g.pick(&[1.0, 0.5]);
                let v = g.near(x, (1 << 12) as f64);
                g.sign(v)
            }),
        ),
        "atan" => u(
            &[0.4375, 0.6875, 1.1875, 2.4375, pow2(66), pow2(-27), 1.0],
            &[0x44100000, 0x3fdc0000, 0x3e400000, 0x3ff30000, 0x3fe60000, 0x40038000, 0x7ff00000],
            -1074,
            1023,
            100.0,
            None,
        ),
        "sinh" => u(
            &[22.0, 710.4758600739439, 709.7822265625, pow2(-28), 1.0],
            &[0x40360000, 0x40862e42, 0x3e300000, 0x3ff00000, 0x7ff00000],
            -80,
            10,
            30.0,
            Some(|g| {
                let x = g.pick(&[22.0, 710.4758600739439, 709.7822265625, 1.0, pow2(-28)]);
                let v = g.near(x, (1 << 10) as f64);
                g.sign(v)
            }),
        ),
        "cosh" => u(
            &[22.0, 710.4758600739439, 709.782712893384, 0.34657359027997264, pow2(-55)],
            &[0x3fd62e43, 0x3c800000, 0x40360000, 0x40862e42, 0x7ff00000],
            -80,
            10,
            30.0,
            Some(|g| {
                let x = g.pick(&[22.0, 710.4758600739439, 709.782712893384, 0.34657359027997264]);
                let v = g.near(x, (1 << 10) as f64);
                g.sign(v)
            }),
        ),
        "tanh" => u(&[22.0, 1.0, pow2(-28)], &[0x40360000, 0x3e300000, 0x3ff00000, 0x7ff00000], -80, 6, 5.0, None),
        "asinh" => u(&[2.0, pow2(28), pow2(-28)], &[0x3e300000, 0x41b00000, 0x40000000, 0x7ff00000], -1074, 1023, 1e3, None),
        "acosh" => unsigned(u(
            &[1.0, 2.0, pow2(28)],
            &[0x3ff00000, 0x41b00000, 0x40000000, 0x7ff00000],
            0,
            1023,
            1e3,
            Some(|g| g.near(1.0, (1 << 20) as f64)),
        )),
        "atanh" => u(
            &[1.0, 0.5, pow2(-28)],
            &[0x3ff00000, 0x3e300000, 0x3fe00000],
            -1074,
            0,
            1.0,
            Some(|g| {
                let x = g.pick(&[1.0, 0.5]);
                let v = g.near(x, (1 << 12) as f64);
                g.sign(v)
            }),
        ),
        "cbrt" => u(
            &[8.0, 27.0, 1e-310, 0.001],
            &[0x00100000, 0x7ff00000],
            -1074,
            1023,
            1e9,
            Some(|g| {
                let k = g.int((1 << 17) as f64) + 1.0;
                let v = g.near(k * k * k, 2.0);
                g.sign(v)
            }),
        ),
        _ => panic!("no unary check named {name}"),
    }
}

fn trig_hard() -> [f64; 7] {
    [
        6381956970095103.0 * pow2(797),
        5.319372648326541e+255,
        1e22,
        1e300,
        pow2(1023),
        pow2(19) * PIO2_HI,
        pow2(20) * PIO2_HI,
    ]
}

fn unary_input(name: &str, c: &Unary, g: &mut Gen) -> f64 {
    let roll = g.int(100.0);
    let mut x;
    if roll < 3.0 {
        x = edge_value(g, c.edges.as_deref());
    } else if roll < 20.0 {
        x = g.raw();
    } else if roll < 30.0 {
        let w = g.pick_u(c.words);
        let v = g.at_high_word(w);
        x = g.sign(v);
    } else if roll < 55.0 {
        let v = g.log_uniform(c.emin, c.emax);
        x = g.sign(v);
    } else if roll < 75.0 {
        x = (g.u01() * 2.0 - 1.0) * c.range;
    } else if c.trig {
        let v = if roll < 78.0 {
            let h = g.pick(&trig_hard());
            g.near(h, 8.0)
        } else {
            g.near_pio2(if roll < 90.0 { 20 } else { 60 })
        };
        x = g.sign(v);
    } else if let Some(hard) = c.hard {
        x = hard(g);
    } else {
        let v = g.log_uniform(-30, 30);
        x = g.sign(v);
    }
    if !c.signed && g.int(20.0) != 0.0 {
        x = x.abs();
    }
    if name == "acosh" && g.int(4.0) != 0.0 && !x.is_nan() {
        x = 1.0 + x.abs();
    }
    x
}

fn pow_input(g: &mut Gen) -> [f64; 2] {
    let y_special = [
        0.0, -0.0, 0.5, -0.5, 1.0, -1.0, 2.0, -2.0, 3.0, -3.0, INF, -INF, NAN, 1.0 / 3.0,
        pow2(53), pow2(53) + 2.0, pow2(31), pow2(31) + 1.0, pow2(64), pow2(63) + pow2(11),
        1e300, -1e300, 5e-324, 1075.0, -1075.0, 1024.0, -1074.0,
    ];
    let x_special = [
        0.0, -0.0, 1.0, -1.0, INF, -INF, NAN, 2.0, -2.0, 10.0, 0.5, -0.5, std::f64::consts::E,
        5e-324, MAX, 1.0 + pow2(-52), 1.0 - pow2(-53), -1.0 - pow2(-52),
    ];
    let roll = g.int(100.0);
    if roll < 10.0 {
        let a = g.raw();
        return [a, g.raw()];
    }
    if roll < 25.0 {
        let a = g.log_uniform(-1074, 1023);
        let u = g.u01() * 2.0 - 1.0;
        return [a, u * g.pick(&[4.0, 50.0, 1000.0])];
    }
    if roll < 35.0 {
        let spread = if g.int(2.0) != 0.0 { 1 << 20 } else { 1 << 4 };
        let a = g.near(1.0, spread as f64);
        let b = g.log_uniform(20, 70);
        return [a, g.sign(b)];
    }
    if roll < 48.0 {
        let a = -g.log_uniform(-30, 30);
        let b = if g.int(2.0) != 0.0 {
            g.int(129.0) - 64.0
        } else {
            let v = g.log_uniform(31, 60).floor();
            g.sign(v)
        };
        return [a, b];
    }
    if roll < 58.0 {
        let v = g.log_uniform(-1074, 1023);
        let a = g.sign(v);
        let y = g.pick(&y_special);
        let spread = if g.int(3.0) == 0.0 { 1.0 } else { 0.0 };
        return [a, g.near(y, spread)];
    }
    if roll < 68.0 {
        let a = g.pick(&x_special);
        let b = if g.int(2.0) != 0.0 { g.pick(&y_special) } else { (g.u01() * 2.0 - 1.0) * 100.0 };
        return [a, b];
    }
    if roll < 83.0 {
        let a = 0.5 + g.u01() * 1.5;
        return [a, (g.u01() * 2.0 - 1.0) * 1100.0];
    }
    if roll < 93.0 {
        let a = g.pick(&[10.0, 2.0, std::f64::consts::E, 1.5, 0.1]);
        let b = if g.int(2.0) != 0.0 { g.int(641.0) - 320.0 } else { (g.u01() * 2.0 - 1.0) * 30.0 };
        return [a, b];
    }
    let a = g.log_uniform(-1074, 1023);
    let b = g.log_uniform(-60, 12);
    [a, g.sign(b)]
}

fn atan2_input(g: &mut Gen) -> [f64; 2] {
    let special = [0.0, -0.0, INF, -INF, NAN, 1.0, -1.0, 5e-324, -5e-324, MAX, -MAX];
    let roll = g.int(100.0);
    if roll < 15.0 {
        let a = g.raw();
        return [a, g.raw()];
    }
    if roll < 50.0 {
        let v = g.log_uniform(-1074, 1023);
        let a = g.sign(v);
        let w = g.log_uniform(-1074, 1023);
        return [a, g.sign(w)];
    }
    if roll < 62.0 {
        let a = g.pick(&special);
        let b = if g.int(2.0) != 0.0 {
            g.pick(&special)
        } else {
            let v = g.log_uniform(-30, 30);
            g.sign(v)
        };
        return [a, b];
    }
    if roll < 70.0 {
        let v = g.log_uniform(-30, 30);
        let a = g.sign(v);
        return [a, g.pick(&special)];
    }
    if roll < 85.0 {
        let v = g.log_uniform(-500, 500);
        let x = g.sign(v);
        let e = g.int(9.0) as i32 - 4 + 60;
        let p = pow2(if g.int(2.0) != 0.0 { e } else { -e });
        return [g.sign(x.abs() * p), x];
    }
    if roll < 92.0 {
        let u = g.u01();
        let a = g.sign(u);
        return [a, g.near(1.0, 2.0)];
    }
    let a = g.u01() * 2.0 - 1.0;
    [a, g.u01() * 2.0 - 1.0]
}

fn hypot_input(g: &mut Gen, n: usize, out: &mut Vec<f64>) {
    let roll = g.int(100.0);
    for _ in 0..n {
        let v = if roll < 15.0 {
            g.raw()
        } else if roll < 25.0 {
            if g.int(3.0) != 0.0 {
                let v = g.log_uniform(-1074, 1023);
                g.sign(v)
            } else {
                g.pick(&[0.0, -0.0, INF, -INF, NAN, MAX, 5e-324])
            }
        } else if roll < 45.0 {
            let v = g.log_uniform(1000, 1023);
            g.sign(v)
        } else if roll < 60.0 {
            let v = g.log_uniform(-1074, -1000);
            g.sign(v)
        } else if roll < 85.0 {
            let v = g.log_uniform(-1074, 1023);
            g.sign(v)
        } else {
            (g.u01() * 2.0 - 1.0) * 100.0
        };
        out.push(v);
    }
}

fn seed_of(name: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for c in name.bytes() {
        h = (h ^ c as u32).wrapping_mul(0x01000193);
    }
    h
}

/// Arguments per input for a check name.
pub fn arity(name: &str) -> usize {
    match name {
        "hypot3" => 3,
        "pow" | "atan2" | "hypot" => 2,
        _ => 1,
    }
}

/// The first `count` inputs for `name`, flattened (`arity(name)` values each).
pub fn inputs(name: &str, count: usize) -> Vec<f64> {
    let mut g = Gen::new(seed_of(name));
    let n = arity(name);
    let mut out = Vec::with_capacity(count * n);
    let c = (n == 1).then(|| unary(name));
    for _ in 0..count {
        match name {
            "pow" => out.extend(pow_input(&mut g)),
            "atan2" => out.extend(atan2_input(&mut g)),
            "hypot" | "hypot3" => hypot_input(&mut g, n, &mut out),
            _ => out.push(unary_input(name, c.as_ref().unwrap(), &mut g)),
        }
    }
    out
}

/// Runs one input through the port's function for `name`.
pub fn apply(name: &str, a: &[f64]) -> f64 {
    match name {
        "exp" => m::exp(a[0]),
        "expm1" => m::expm1(a[0]),
        "log" => m::ln(a[0]),
        "log1p" => m::ln_1p(a[0]),
        "log2" => m::log2(a[0]),
        "log10" => m::log10(a[0]),
        "sin" => m::sin(a[0]),
        "cos" => m::cos(a[0]),
        "tan" => m::tan(a[0]),
        "asin" => m::asin(a[0]),
        "acos" => m::acos(a[0]),
        "atan" => m::atan(a[0]),
        "sinh" => m::sinh(a[0]),
        "cosh" => m::cosh(a[0]),
        "tanh" => m::tanh(a[0]),
        "asinh" => m::asinh(a[0]),
        "acosh" => m::acosh(a[0]),
        "atanh" => m::atanh(a[0]),
        "cbrt" => m::cbrt(a[0]),
        "pow" => m::pow(a[0], a[1]),
        "atan2" => m::atan2(a[0], a[1]),
        "hypot" | "hypot3" => m::hypot(a),
        _ => panic!("no check named {name}"),
    }
}

/// The two-lane result hash: NaN counted as one value, the sign of zero counted.
#[derive(Clone)]
pub struct Hash {
    h1: u32,
    h2: u32,
}

impl Default for Hash {
    fn default() -> Self {
        Hash { h1: 0x2545f491, h2: 0x6a09e667 }
    }
}

impl Hash {
    pub fn add(&mut self, x: f64) {
        let (hi, lo) = if x.is_nan() {
            (0x7ff80000, 0)
        } else {
            let b = x.to_bits();
            ((b >> 32) as u32, b as u32)
        };
        self.h1 = (self.h1 ^ lo).wrapping_mul(0x85ebca6b).rotate_left(13);
        self.h2 = (self.h2 ^ hi).wrapping_mul(0xc2b2ae35).rotate_left(17);
        self.h1 = self.h1.wrapping_add(self.h2);
        self.h2 = self.h2.wrapping_add(self.h1.wrapping_mul(5));
    }

    pub fn value(&self) -> u64 {
        ((self.h1 as u64) << 32) | self.h2 as u64
    }

    pub fn hex(&self) -> String {
        format!("{:016x}", self.value())
    }
}

/// The port's result hash over the first `count` inputs of `name`.
pub fn run(name: &str, count: usize) -> String {
    let n = arity(name);
    let mut h = Hash::default();
    for args in inputs(name, count).chunks(n) {
        h.add(apply(name, args));
    }
    h.hex()
}

/// Inputs per function for the cross-target check.
pub const TARGET_COUNT: usize = 20_000;

/// One hash over all 23 checks at `count` inputs each: equal on two targets means every
/// function gave the same bits on both. Build targets compare `combined_hash(TARGET_COUNT)`.
pub fn combined_hash(count: usize) -> u64 {
    let mut h = Hash::default();
    for name in names() {
        for args in inputs(name, count).chunks(arity(name)) {
            h.add(apply(name, args));
        }
    }
    h.value()
}
