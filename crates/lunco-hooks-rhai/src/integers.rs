//! Exact comparisons for unsigned ABI counters and Rhai's signed integers.

use rhai::Engine;

pub(super) fn register_integer_functions(engine: &mut Engine) {
    macro_rules! comparison {
        ($operator:tt) => {
            engine.register_fn(stringify!($operator), |left: u64, right: i64| {
                i128::from(left) $operator i128::from(right)
            });
            engine.register_fn(stringify!($operator), |left: i64, right: u64| {
                i128::from(left) $operator i128::from(right)
            });
        };
    }
    comparison!(==);
    comparison!(!=);
    comparison!(<);
    comparison!(<=);
    comparison!(>);
    comparison!(>=);
}

#[cfg(test)]
mod tests {
    #[test]
    fn mixed_integer_comparisons_preserve_sign_and_all_unsigned_bits() {
        let mut engine = rhai::Engine::new();
        super::register_integer_functions(&mut engine);
        let mut scope = rhai::Scope::new();
        scope.push("zero", 0_u64);
        scope.push("one", 1_u64);
        scope.push("signed_max", i64::MAX as u64);
        scope.push("next", (i64::MAX as u64) + 1);
        scope.push("max", u64::MAX);
        assert!(
            engine
                .eval_with_scope::<bool>(
                    &mut scope,
                    r#"
            zero == 0 && 0 == zero && zero != -1 && -1 != zero
                && zero > -1 && -1 < zero && one > 0 && 0 < one
                && one >= 1 && 1 <= one && one <= 1 && 1 >= one
                && signed_max == 9223372036854775807
                && next > 9223372036854775807 && max > 9223372036854775807
                && 9223372036854775807 < next && 9223372036854775807 < max
                && !(max < -1) && !(-1 > max) && !(max == -1)
        "#
                )
                .expect("mixed comparisons remain exact across signed boundaries")
        );
    }
}
