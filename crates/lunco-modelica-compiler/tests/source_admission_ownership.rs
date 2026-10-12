//! Compiler source-admission ownership checks.

use std::path::Path;

fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(index) => &line[..index],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// User overlays enter through one owner so source capture and eviction agree.
#[test]
fn user_source_is_seated_through_one_owner() {
    let compiler_lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let text = lunco_storage::read_text_file_sync(&compiler_lib).expect("compiler lib.rs readable");

    let seats: Vec<String> = code_only(&text)
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            l.contains("session.update_document(")
                || l.contains("session.add_document(")
                || l.contains("session.add_in_memory_parsed_batch(")
        })
        .map(|(i, l)| format!("{}:{}: {}", compiler_lib.display(), i + 1, l.trim()))
        .collect();

    // One parsed batch publishes all validated user overlays. Library roots
    // remain owned by their separate source-set installation boundary.
    assert_eq!(
        seats.len(),
        1,
        "user overlays must enter through one parsed-source batch; sites: {}",
        seats.join("\n  ")
    );
    assert!(
        seats[0].contains("session.add_in_memory_parsed_batch("),
        "the admission owner must install parser-produced source/definition pairs"
    );
}
