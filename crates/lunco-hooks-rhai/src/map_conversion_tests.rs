use super::*;

#[test]
fn map_conversion_preserves_nested_values_and_last_duplicate() {
    let value = HookValue::Map(vec![
        ("z".into(), HookValue::Int(1)),
        ("µ".into(), HookValue::UInt(u64::MAX)),
        (
            "a".into(),
            HookValue::Array(vec![HookValue::Bytes(vec![0, 255]), HookValue::Unit]),
        ),
        ("z".into(), HookValue::Float(-0.0)),
        (
            "nested".into(),
            HookValue::Map(vec![
                ("value".into(), HookValue::Bool(false)),
                ("value".into(), HookValue::Str("current".into())),
            ]),
        ),
    ]);
    let dynamic = hook_to_dynamic(&value);
    let map = dynamic.as_map_ref().unwrap();
    assert_eq!(map.len(), 4);
    assert_eq!(
        map.keys().map(|key| key.as_str()).collect::<Vec<_>>(),
        ["a", "nested", "z", "µ"]
    );
    assert_eq!(map["z"].as_float().unwrap().to_bits(), (-0.0_f64).to_bits());
    assert_eq!(map["µ"].clone_cast::<u64>(), u64::MAX);
    assert_eq!(
        dynamic_to_hook(&map["a"]).unwrap(),
        HookValue::Array(vec![HookValue::Bytes(vec![0, 255]), HookValue::Unit])
    );
    assert_eq!(
        dynamic_to_hook(&map["nested"]).unwrap(),
        HookValue::map([("value", HookValue::str("current"))])
    );
    assert!(
        hook_to_dynamic(&HookValue::Map(vec![]))
            .as_map_ref()
            .unwrap()
            .is_empty()
    );
    for text in [String::new(), "µ".repeat(256), "x".repeat(32)] {
        let input = HookValue::Str(text);
        assert_eq!(dynamic_to_hook(&hook_to_dynamic(&input)).unwrap(), input);
    }
}

#[test]
#[ignore = "manual complete typed-map conversion timing"]
fn map_conversion_diagnostic() {
    for width in [8, 128, 2048] {
        for order in ["sorted", "reverse", "permuted"] {
            let values = (0..width)
                .map(|position| {
                    let index = match order {
                        "sorted" => position,
                        "reverse" => width - 1 - position,
                        _ => (position * 37) % width,
                    };
                    (
                        format!("signal_{index:06}"),
                        HookValue::map([
                            ("value", HookValue::Float(index as f64)),
                            ("unit", HookValue::str("m/s")),
                        ]),
                    )
                })
                .collect();
            let input = HookValue::Map(values);
            let check = hook_to_dynamic(&input);
            let map = check.as_map_ref().unwrap();
            assert_eq!(map.len(), width);
            for index in 0..width {
                assert_eq!(
                    dynamic_to_hook(&map[format!("signal_{index:06}").as_str()]).unwrap(),
                    HookValue::map([
                        ("unit", HookValue::str("m/s")),
                        ("value", HookValue::Float(index as f64)),
                    ])
                );
            }
            let iterations = 262_144 / width;
            let started = std::time::Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(hook_to_dynamic(std::hint::black_box(&input)));
            }
            println!(
                "MAP_CONVERSION width={width} order={order} iterations={iterations} ns={}",
                started.elapsed().as_nanos()
            );
        }
    }
}
