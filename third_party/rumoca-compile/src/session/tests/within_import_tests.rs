use super::*;

#[test]
fn within_import_resolves_enclosing_package_alias_in_both_compile_paths() {
    let sources = [
        (
            "HeaderImports/package.mo",
            "package HeaderImports import Alias = HeaderImports.Shared; type Shared = Real; end HeaderImports;",
        ),
        (
            "HeaderImports/Child.mo",
            "within HeaderImports; model Child output Alias answer(unit=\"K\")=3; end Child;",
        ),
    ];
    for reverse in [false, true] {
        let mut session = Session::default();
        for index in if reverse { [1, 0] } else { [0, 1] } {
            let (uri, source) = sources[index];
            session.add_document(uri, source).expect("valid source");
        }
        let full = session.compile_model("HeaderImports.Child");
        assert!(
            full.is_ok(),
            "full compile, reverse={reverse}: {:?}",
            full.err()
        );
        let strict = session.compile_model_strict("HeaderImports.Child");
        assert!(
            strict.is_ok(),
            "strict reachable compile, reverse={reverse}: {:?}",
            strict.err()
        );
    }
}
