use bluing_syntax::{Code, Options, check};

fn codes(text: &str, options: &Options) -> Vec<Code> {
    check(text, options).into_iter().map(|d| d.code).collect()
}

fn default_codes(text: &str) -> Vec<Code> {
    codes(text, &Options::default())
}

#[test]
fn ordinary_rust_is_accepted() {
    let text = r#"
        struct Enemy { hp: i32, name: String }

        impl Enemy {
            fn take_hit(&mut self, dmg: i32) -> bool {
                self.hp -= dmg;
                self.hp <= 0
            }
            fn name(&self) -> &str { &self.name }
        }

        async fn patrol(enemy: &mut Enemy) {
            let names: Vec<&str> = vec!["a", "b"].into_iter().collect();
            println!("{} {:?}", enemy.name(), names);
        }
    "#;
    assert_eq!(default_codes(text), []);
}

#[test]
fn syntax_errors_are_reported() {
    // The parser recovers and may report more than one error.
    let codes = default_codes("fn main( {}");
    assert!(!codes.is_empty() && codes.iter().all(|&c| c == Code::Syntax));
}

#[test]
fn every_form_of_unsafe_is_gated() {
    let text = r#"
        unsafe fn a() {}
        fn b() { unsafe {} }
        unsafe trait C {}
        unsafe impl C for () {}
        #[unsafe(no_mangle)] fn d() {}
        fn e() { vec![unsafe { 1 }]; }
    "#;
    assert_eq!(default_codes(text), [Code::Unsafe; 6]);
}

#[test]
fn unsafe_is_accepted_when_allowed() {
    let options = Options { allow_unsafe: true };
    assert_eq!(codes("fn b() { unsafe {} }", &options), []);
}

#[test]
fn unsafe_in_strings_comments_and_raw_identifiers_is_not_flagged() {
    let text = r#"
        // unsafe
        fn r#unsafe() { let _ = "unsafe"; }
    "#;
    assert_eq!(default_codes(text), []);
}

#[test]
fn ffi_is_rejected_even_with_unsafe_allowed() {
    let options = Options { allow_unsafe: true };
    let text = r#"
        unsafe extern "C" { fn puts(s: *const u8) -> i32; }
        extern "C" fn callback() {}
        fn take(f: extern "C" fn()) {}
    "#;
    assert_eq!(codes(text, &options), [Code::Ffi; 3]);
}

#[test]
fn inline_asm_is_rejected() {
    let options = Options { allow_unsafe: true };
    let text = r#"
        fn f() { unsafe { core::arch::asm!("nop") } }
        std::arch::global_asm!("");
    "#;
    assert_eq!(codes(text, &options), [Code::InlineAsm; 2]);
}

#[test]
fn macro_rules_is_not_supported_yet() {
    assert_eq!(
        default_codes("macro_rules! m { () => {} }"),
        [Code::MacroRules]
    );
}

#[test]
fn diagnostics_are_in_source_order() {
    let text = "macro_rules! m { () => {} }\nfn b() { unsafe {} }\nextern \"C\" fn c() {}";
    assert_eq!(
        default_codes(text),
        [Code::MacroRules, Code::Unsafe, Code::Ffi]
    );
}
