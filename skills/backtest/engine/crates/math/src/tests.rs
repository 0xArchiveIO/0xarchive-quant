use super::*;

#[test]
fn equals_x64_v8_bit_for_bit() {
    let mut wrong = Vec::new();
    for (name, golden) in check::GOLDEN {
        let got = check::run(name, 50_000);
        if got != golden {
            wrong.push(format!("{name}: {got}, want {golden}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

fn same(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
}

#[test]
fn special_values() {
    let nan = f64::NAN;
    let inf = f64::INFINITY;
    assert!(pow(1.0, inf).is_nan(), "ECMAScript, not C: 1 ** inf is NaN");
    assert!(pow(-1.0, -inf).is_nan());
    assert_eq!(pow(nan, 0.0), 1.0);
    assert!(same(pow(-0.0, 3.0), -0.0));
    assert_eq!(pow(-0.0, -3.0), -inf);
    assert_eq!(pow(-inf, -2.0), 0.0);
    assert!(same(atan2(-0.0, 0.0), -0.0));
    assert_eq!(atan2(0.0, -0.0), std::f64::consts::PI);
    assert_eq!(hypot(&[inf, nan]), inf);
    assert!(same(hypot(&[-0.0]), 0.0));
    assert!(same(hypot(&[]), 0.0));
    assert!(same(expm1(-0.0), -0.0));
    assert_eq!(exp(1.0), std::f64::consts::E);
    assert_eq!(ln(0.0), -inf);
    assert!(ln(-1.0).is_nan());
    assert_eq!(hypot(&[3.0, 4.0, 12.0]), 13.0);
    // V8's fdlibm value; Apple's libm gives 1.0986122886681098.
    assert_eq!(ln(3.0), 1.0986122886681096);
}
