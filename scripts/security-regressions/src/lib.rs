#[cfg(test)]
mod tests {
    use ark_relations::r1cs::{ConstraintLayer, ConstraintTrace, TracingMode};
    use glib::prelude::ToVariant;
    use std::hint::black_box;
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    #[test]
    fn optimized_variant_string_iterators_keep_valid_out_pointers() {
        // The original defect dereferences NULL after compiler optimization.
        // Exercise every affected Iterator/DoubleEndedIterator entry point.
        let expected = ["alpha", "", "Boise", "\u{03bb}\u{1f30e}", "omega"];
        for _ in 0..128 {
            let variant = black_box(expected.to_variant());
            assert_eq!(
                variant.array_iter_str().unwrap().collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                variant.array_iter_str().unwrap().rev().collect::<Vec<_>>(),
                expected.into_iter().rev().collect::<Vec<_>>()
            );
            let mut iter = variant.array_iter_str().unwrap();
            assert_eq!(iter.next(), Some("alpha"));
            assert_eq!(iter.next_back(), Some("omega"));
            assert_eq!(iter.nth(1), Some("Boise"));
            assert_eq!(iter.nth_back(0), Some("\u{03bb}\u{1f30e}"));
            assert_eq!(iter.next(), None);
            assert_eq!(variant.array_iter_str().unwrap().last(), Some("omega"));
            assert_eq!(variant.array_iter_str().unwrap().nth(99), None);
            assert_eq!(variant.array_iter_str().unwrap().nth_back(99), None);
        }
        let empty: [&str; 0] = [];
        assert_eq!(empty.to_variant().array_iter_str().unwrap().next(), None);
    }

    #[test]
    fn ark_constraint_layer_records_nested_spans_with_current_subscriber() {
        let subscriber = Registry::default().with(ConstraintLayer::<Registry>::new(
            TracingMode::OnlyConstraints,
        ));
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let outer = tracing::info_span!(target: "r1cs", "outer_constraint");
        let _outer = outer.enter();
        let inner = tracing::info_span!(target: "r1cs", "inner_constraint");
        let _inner = inner.enter();
        let trace =
            ConstraintTrace::capture().expect("an entered constraint span must be captured");
        let path = trace.path();
        assert_eq!(
            path.iter().map(|step| step.name).collect::<Vec<_>>(),
            ["outer_constraint", "inner_constraint"]
        );
        assert!(path
            .iter()
            .all(|step| step.line > 0 && step.file.ends_with("lib.rs")));
        let rendered = trace.to_string();
        assert!(rendered.contains("outer_constraint") && rendered.contains("inner_constraint"));
    }

    #[test]
    fn ark_constraint_layer_keeps_constraint_filtering() {
        let subscriber =
            Registry::default().with(ConstraintLayer::<Registry>::new(TracingMode::NoConstraints));
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let excluded = tracing::info_span!(target: "r1cs", "excluded_constraint");
        let _entered = excluded.enter();
        assert!(ConstraintTrace::capture().is_none());
    }
}
