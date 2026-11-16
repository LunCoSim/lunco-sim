//! CSV serialization shared by plots and experiments. Missing samples stay empty;
//! exports never resample or extend a series beyond its measured timestamps.

use bevy::prelude::*;

/// Append `s` as a single CSV field to `out`, RFC-4180 quoting when it
/// contains a comma, double-quote, or newline (doubling embedded quotes).
pub fn push_csv_field(out: &mut String, s: &str) {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        out.push('"');
        for c in s.chars() {
            if c == '"' {
                out.push('"');
            }
            out.push(c);
        }
        out.push('"');
    } else {
        out.push_str(s);
    }
}

#[derive(Component)]
struct PendingCsv {
    name: String,
    task: bevy::tasks::Task<Result<String, String>>,
}

/// Serialize an admitted immutable snapshot off the UI thread.
pub fn export_columns(
    world: &mut World,
    suggested_name: String,
    columns: Vec<(String, Vec<(f64, f64)>)>,
) {
    export_snapshot(world, suggested_name, move || columns_csv(&columns));
}

/// Encode an admitted immutable result without blocking the UI thread.
pub fn export_snapshot(
    world: &mut World,
    name: String,
    encode: impl FnOnce() -> Result<String, String> + Send + 'static,
) {
    let Some(limit) = world
        .get_resource::<lunco_workbench_file_ops::ExportLimits>()
        .map(|limits| limits.max_pending)
    else {
        world.commands().trigger(lunco_core::RuntimeError {
            name: "export-failed".into(),
            message: "Export workflow is not installed".into(),
        });
        return;
    };
    if limit == 0 || world.query::<&PendingCsv>().iter(world).count() >= limit {
        world.commands().trigger(lunco_core::RuntimeError {
            name: "export-failed".into(),
            message:
                "Export preparation queue is full. Wait for an existing export before retrying."
                    .into(),
        });
        return;
    }
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move { encode() });
    world.spawn(PendingCsv { name, task });
}

fn finish_csv(mut jobs: Query<(Entity, &mut PendingCsv)>, mut commands: Commands) {
    for (entity, mut job) in &mut jobs {
        let Some(result) = bevy::tasks::futures_lite::future::block_on(
            bevy::tasks::futures_lite::future::poll_once(&mut job.task),
        ) else {
            continue;
        };
        match result {
            Ok(content) => commands.trigger(lunco_workbench_file_ops::ExportText {
                suggested_name: job.name.clone(),
                content,
                path: String::new(),
            }),
            Err(message) => commands.trigger(lunco_core::RuntimeError {
                name: "export-failed".into(),
                message,
            }),
        }
        commands.entity(entity).try_despawn();
    }
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(Update, finish_csv);
}

/// Serialize sparse series on their union of exact native timestamps.
pub fn columns_csv(columns: &[(String, Vec<(f64, f64)>)]) -> Result<String, String> {
    let mut times = Vec::new();
    for (_, samples) in columns {
        if samples.iter().any(|(time, _)| !time.is_finite()) {
            return Err("CSV export has a non-finite sample timestamp".into());
        }
        if samples.windows(2).any(|pair| pair[0].0 > pair[1].0) {
            return Err("CSV export sample timestamps are not ordered".into());
        }
        times.extend(samples.iter().map(|(time, _)| *time));
    }
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| *a == *b);
    let mut csv = String::from("time (s)");
    for (name, _) in columns {
        csv.push(',');
        push_csv_field(&mut csv, name);
    }
    csv.push('\n');
    let mut cursors = vec![0usize; columns.len()];
    for time in times {
        csv.push_str(&time.to_string());
        for (index, (_, samples)) in columns.iter().enumerate() {
            csv.push(',');
            let mut value = None;
            while cursors[index] < samples.len() && samples[cursors[index]].0 <= time {
                let (sample_time, sample_value) = samples[cursors[index]];
                if sample_time == time {
                    value = sample_value.is_finite().then_some(sample_value);
                }
                cursors[index] += 1;
            }
            if let Some(value) = value {
                csv.push_str(&value.to_string());
            }
        }
        csv.push('\n');
    }
    Ok(csv)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_csv_preserves_sample_validity() {
        let columns = vec![
            ("a".into(), vec![(0.0, 1.0), (2.0, 3.0)]),
            ("b,unit".into(), vec![(1.0, 4.0), (3.0, 5.0)]),
        ];
        assert_eq!(
            columns_csv(&columns).unwrap(),
            "time (s),a,\"b,unit\"\n0,1,\n1,,4\n2,3,\n3,,5\n"
        );
        assert_eq!(
            columns_csv(&[("a".into(), vec![(0.0, 1.0), (0.0, f64::NAN)])]).unwrap(),
            "time (s),a\n0,\n"
        );
        assert!(columns_csv(&[("bad".into(), vec![(f64::NAN, 1.0)])]).is_err());
        assert!(columns_csv(&[("bad".into(), vec![(2.0, 1.0), (1.0, 2.0)])]).is_err());
    }
}
