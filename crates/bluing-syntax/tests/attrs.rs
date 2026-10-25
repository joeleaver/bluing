use bluing_base::{Code, FileId};
use bluing_syntax::{Options, parse};

fn codes(text: &str, options: &Options) -> Vec<Code> {
    parse(FileId(0), text, options)
        .diagnostics
        .into_iter()
        .map(|d| d.code)
        .collect()
}

fn default_codes(text: &str) -> Vec<Code> {
    codes(text, &Options::default())
}

#[test]
fn inert_attributes_are_accepted() {
    let text = r#"
        #![allow(dead_code)]
        /// Doc comment.
        #[doc = "more docs"]
        #[doc(hidden)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        #[repr(C)]
        #[must_use = "why"]
        #[deprecated(note = "old")]
        struct S;

        #[inline(always)]
        #[cold]
        #[warn(clippy::all)]
        #[rustfmt::skip]
        fn f() { #[expect(unused_variables)] let x = 1; }

        #[repr(transparent)]
        struct T(i32);
    "#;
    assert_eq!(default_codes(text), []);
}

#[test]
fn unknown_attributes_are_rejected() {
    let text = r#"
        #[no_mangle] fn a() {}
        #[test] fn b() {}
        #[non_exhaustive] struct C;
        #[serde::rename] struct D;
    "#;
    assert_eq!(default_codes(text), [Code::Attribute; 4]);
}

#[test]
fn malformed_known_attributes_are_rejected() {
    assert_eq!(default_codes("#[allow] fn a() {}"), [Code::Attribute]);
    assert_eq!(
        default_codes("#[derive = \"Debug\"] struct S;"),
        [Code::Attribute]
    );
}

#[test]
fn only_builtin_derives_are_accepted() {
    let codes = default_codes("#[derive(Debug, Serialize)] struct S;");
    assert_eq!(codes, [Code::Attribute]);
}

#[test]
fn derive_paths_are_not_supported_yet() {
    let codes = default_codes("#[derive(std::fmt::Debug)] struct S;");
    assert_eq!(codes, [Code::NotYetSupported]);
}

#[test]
fn only_repr_c_and_transparent_are_accepted() {
    for repr in ["u8", "packed", "align(4)", "C, u8"] {
        let text = format!("#[repr({repr})] enum E {{ A }}");
        assert_eq!(default_codes(&text), [Code::Attribute], "repr({repr})");
    }
}

#[test]
fn cfg_and_path_are_not_supported_yet() {
    let text = r#"
        #[cfg(test)] fn a() {}
        #[cfg_attr(test, inline)] fn b() {}
        #[path = "x.blu"] mod c;
    "#;
    assert_eq!(default_codes(text), [Code::NotYetSupported; 3]);
}

#[test]
fn unsafe_attributes_are_gated_and_checked() {
    let options = Options {
        allow_unsafe: true,
        ..Options::default()
    };
    // The keyword is allowed, but `no_mangle` is still not an allowed attribute.
    assert_eq!(
        codes("#[unsafe(no_mangle)] fn a() {}", &options),
        [Code::Attribute]
    );
}

#[test]
fn lang_attributes_require_library_mode() {
    let text = "#[lang = \"sized\"] trait Sized {}";
    assert_eq!(default_codes(text), [Code::Attribute]);
    let library = Options {
        library: true,
        ..Options::default()
    };
    assert_eq!(codes(text, &library), []);
}
