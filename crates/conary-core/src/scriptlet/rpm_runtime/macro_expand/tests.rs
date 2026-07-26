// conary-core/src/scriptlet/rpm_runtime/macro_expand/tests.rs

use super::RpmMacroEngine;
use crate::ccs::native_lifecycle::{
    RpmCriticality, RpmHeaderContext, RpmMacroContext, RpmMacroDefinition,
    RpmMacroDefinitionSource, RpmProgram, RpmRuntimeMetadata,
};
use crate::scriptlet::test_support::materialized_root;
use crate::scriptlet::{PackageFormat, SandboxMode, ScriptletExecutor};

fn engine(definitions: &[(&str, &str)]) -> RpmMacroEngine {
    RpmMacroEngine::from_context(&RpmMacroContext {
        definitions: definitions
            .iter()
            .map(|(name, body)| RpmMacroDefinition {
                name: (*name).to_string(),
                body: (*body).to_string(),
                source: RpmMacroDefinitionSource::PackageHeader,
            })
            .collect(),
    })
}

fn runtime() -> RpmRuntimeMetadata {
    RpmRuntimeMetadata {
        program: RpmProgram::External,
        body_transforms: Vec::new(),
        critical: false,
        criticality: RpmCriticality::WarningOnly,
        raw_flags: 0,
        unknown_flags: 0,
        install_prefixes: Vec::new(),
        macro_context: RpmMacroContext::default(),
        header_context: RpmHeaderContext::default(),
        package_rpm_version: Some("6.0.0".to_string()),
    }
}

#[test]
fn expands_names_conditionals_and_preserves_unknown_calls() {
    let engine = engine(&[("name", "demo"), ("this", "that"), ("that_that", "dynamic")]);
    assert_eq!(
        engine
            .expand("%{name}:%{?name:yes}:%{!?missing:no}:%{missing}")
            .expect("macro expansion"),
        "demo:yes:no:%{missing}"
    );
    assert_eq!(
        engine
            .expand("%{expand:%{%{this}_that}}")
            .expect("nested macro name"),
        "dynamic"
    );
}

#[test]
fn expands_parametric_automatic_macros_and_core_builtins() {
    let engine = engine(&[]);
    engine
        .define_spec("foo(zb:) %0:%#:%1:%2:%{-z}:%{-b*}", false, false, 0)
        .expect("parametric macro");
    assert_eq!(
        engine
            .expand("%foo one two -z -b3")
            .expect("parametric expansion"),
        "foo:2:one:two:-z:3"
    );
    assert_eq!(
        engine
            .expand("%{basename:/data/demo.tar.gz}:%{suffix:/data/demo.tar.gz}")
            .expect("builtins"),
        "demo.tar.gz:gz"
    );
}

#[test]
fn matches_upstream_parametric_macro_fixtures() {
    let engine = engine(&[("bar", "yyy")]);
    engine
        .define_spec("foo() 1:%1 2:%2", false, false, 0)
        .expect("parametric macro");
    assert_eq!(
        engine.expand("%foo %{nil} bar").expect("nil argument"),
        "1:bar 2:%2"
    );
    assert_eq!(
        engine.expand("%foo %bar").expect("expanded arg"),
        "1:yyy 2:%2"
    );
    assert_eq!(
        engine.expand("%foo %%bar").expect("escaped arg"),
        "1:%bar 2:%2"
    );

    engine
        .define_spec("quoted() %#:%{?1:\"%1\"}%{?2: \"%2\"}", false, false, 0)
        .expect("quoted macro");
    assert_eq!(engine.expand("%quoted 1").expect("one arg"), "1:\"1\"");
    assert_eq!(
        engine
            .expand("%quoted %{quote:   2 3  5} %{quote:%{nil}}")
            .expect("quoted args"),
        "2:\"   2 3  5\" \"\""
    );
    assert_eq!(
        engine.expand("%quoted %{quote 1 2}").expect("word quote"),
        "2:\"1\" \"2\""
    );

    engine
        .define_spec("inner() %#:%1", false, false, 0)
        .expect("inner macro");
    engine
        .define_spec("outer() %inner %**", false, false, 0)
        .expect("outer macro");
    assert_eq!(
        engine
            .expand("%outer %{quote:a b}")
            .expect("quoted forwarding"),
        "1:a b"
    );
    assert_eq!(
        engine.expand("%quoted 1 \\\nbar").expect("escaped newline"),
        "2:\"1\" \"\\\"bar"
    );
}

#[test]
fn matches_upstream_lua_macro_argument_and_macros_table_fixtures() {
    let engine = engine(&[]);
    engine
        .define_spec("foo(a:) %{lua:print(opt.a, arg[1])}", false, false, 0)
        .expect("Lua option macro");
    engine
        .define_spec(
            "qtst() %{lua:for i=1, #arg do io.write(' '..i..':'..arg[i]) end}",
            false,
            false,
            0,
        )
        .expect("Lua argument macro");

    assert_eq!(
        engine.expand("%foo -a3 5").expect("Lua macro arguments"),
        "3\t5\n"
    );
    assert_eq!(
        engine
            .expand("%{lua:print(macros.with('zap'), macros.without('zap'))}")
            .expect("builtin macro closures"),
        "0\t1\n"
    );
    assert_eq!(
        engine
            .expand("%{lua:print(macros.qtst({'a', '', 'c'}))}")
            .expect("exact table arguments"),
        " 1:a 2: 3:c\n"
    );
    assert_eq!(
        engine
            .expand("%{lua:macros.aaa='bbb'; print(macros.aaa); macros.aaa=nil; print(macros.aaa)}")
            .expect("mutable macro table"),
        "bbb\nnil\n"
    );
}

#[test]
fn matches_upstream_lua_macro_definition_introspection_and_argument_quoting() {
    let engine = engine(&[]);
    assert_eq!(
        engine
            .expand(
                "%{lua:rpm.define('pair() %1:%2'); print(rpm.isdefined('pair')); \
                 print(macros.pair({'one', 'two'})); \
                 local a=rpm.splitargs('1 2 3'); a[2]=' '..a[2]..' '; \
                 io.write(rpm.unsplitargs(a))}"
            )
            .expect("Lua RPM macro APIs"),
        "true\ttrue\none:two\n1  2  3"
    );
    assert_eq!(
        engine
            .expand("%{lua:return string.reverse('hello')}")
            .expect("Lua return values"),
        "olleh\n"
    );
}

#[test]
fn matches_upstream_pure_builtin_fixtures() {
    let engine = engine(&[]);
    assert_eq!(
        engine
            .expand("%{dirname:/foo/foobar/}:%{basename:foobar/}")
            .expect("path builtins"),
        "/foo:foobar"
    );
    assert_eq!(
        engine
            .expand("%{shrink:  h e  l   lo  }:%{url2path:http://example/a/b}")
            .expect("pure builtins"),
        "h e l lo:/a/b"
    );
    assert_eq!(
        engine
            .expand("%sub cd6317ba61e6c27b92d6bbdf2702094ff3c0c732 1 7")
            .expect("substring"),
        "cd6317b"
    );
    assert_eq!(engine.expand("%upper abba").expect("upper"), "ABBA");
    assert_eq!(engine.expand("%rep ai 2").expect("repeat"), "aiai");
    assert_eq!(engine.expand("%reverse live").expect("reverse"), "evil");
    assert_eq!(engine.expand("%gsub aaa a b 2").expect("gsub"), "bba");
}

#[test]
fn define_directives_preserve_rpm_scope_expansion_and_modifier_semantics() {
    let engine = engine(&[("source", "one")]);
    assert_eq!(
        engine
            .expand("%define eager %{source}\n%define literal<l> %{source}\n")
            .expect("definitions"),
        ""
    );
    engine
        .define_body("source".to_string(), "two".to_string())
        .expect("replace source");
    assert_eq!(
        engine
            .expand("%{eager}:%{literal}")
            .expect("definition semantics"),
        "two:%{source}"
    );

    engine
        .define_spec("-e expanded %{source}", false, false, 0)
        .expect("eager definition");
    engine
        .define_spec("cached<o> %{source}", false, false, 0)
        .expect("one-shot definition");
    assert_eq!(engine.expand("%{expanded}").expect("eager"), "two");
    assert_eq!(engine.expand("%{cached}").expect("first use"), "two");
    engine
        .define_body("source".to_string(), "three".to_string())
        .expect("replace source again");
    assert_eq!(engine.expand("%{cached}").expect("cached use"), "two");
}

#[test]
fn quote_builtin_preserves_one_argument_across_outer_parameter_splitting() {
    let engine = engine(&[]);
    engine
        .define_spec("foo() %#:%1:%2", false, false, 0)
        .expect("parametric macro");
    assert_eq!(
        engine
            .expand("%foo %{quote:   2 3  5} %{quote:%{nil}}")
            .expect("quoted argument expansion"),
        "2:   2 3  5:"
    );
}

#[test]
fn evaluates_integer_string_and_version_expressions() {
    let engine = engine(&[]);
    assert_eq!(engine.expand("%[4*1024]").expect("integer"), "4096");
    assert_eq!(
        engine
            .expand("%{expr:0 ? \"no\" : \"yes\"}")
            .expect("ternary"),
        "yes"
    );
    assert_eq!(engine.expand("%[v\"2\" > v\"1\"]").expect("version"), "1");
    assert_eq!(
        engine.expand("%[%{?missing}]").expect("empty macro value"),
        "0"
    );
    assert_eq!(
        engine
            .expand("%[lua:string.reverse(\"hello\")]")
            .expect("Lua expression function"),
        "olleh"
    );
    assert!(
        engine
            .expand("%{expr:1 || \"wrong-type\"}")
            .expect_err("short-circuit still checks operand types")
            .to_string()
            .contains("types must match")
    );
}

#[test]
fn shell_expansion_uses_target_runtime_environment_and_rpm_newline_rules() {
    let Some(root) = materialized_root(
        "scriptlet::rpm_runtime::macro_expand::tests::shell_expansion_uses_target_runtime_environment_and_rpm_newline_rules",
        &["/bin/sh"],
    ) else {
        return;
    };
    let executor = ScriptletExecutor::new(root.path(), "demo", "1", PackageFormat::Rpm)
        .with_sandbox_mode(SandboxMode::Always);
    let runtime = runtime();
    let engine = RpmMacroEngine::from_runtime(
        &executor,
        &[("RPM_TEST_VALUE".to_string(), "target".to_string())],
        &runtime,
    );

    assert_eq!(
        engine
            .expand("%(printf '%s\\n\\r\\n' \"$RPM_TEST_VALUE\"; exit 7)")
            .expect("RPM ignores the command status"),
        "target"
    );
    assert_eq!(engine.expand("%{verbose}").expect("verbose value"), "0");
    assert_eq!(
        engine
            .expand("%{rpmversion}")
            .expect("emulated RPM version"),
        "6.1.90"
    );
    assert_eq!(
        engine.expand("%verbose foo").expect("unbraced verbose"),
        "0 foo"
    );
}

#[test]
fn shell_expansion_never_falls_back_to_the_conversion_host() {
    let root = tempfile::tempdir().expect("target root");
    let executor = ScriptletExecutor::new(root.path(), "demo", "1", PackageFormat::Rpm);
    let runtime = runtime();
    let engine = RpmMacroEngine::from_runtime(&executor, &[], &runtime);

    let error = engine
        .expand("%(printf host)")
        .expect_err("missing target shell must fail");
    assert!(error.to_string().contains("missing from target root"));
}

#[test]
fn lua_macro_uses_the_same_target_root_runtime_as_embedded_lua() {
    let root = tempfile::tempdir().expect("target root");
    std::fs::create_dir_all(root.path().join("usr/lib/rpm/lua")).expect("module directory");
    std::fs::create_dir_all(root.path().join("data")).expect("data directory");
    std::fs::write(root.path().join("data/value"), "payload").expect("target value");
    std::fs::write(
        root.path().join("usr/lib/rpm/lua/demo.lua"),
        "return { value = 42 }",
    )
    .expect("target module");
    let executor = ScriptletExecutor::new(root.path(), "demo", "1", PackageFormat::Rpm);
    let runtime = runtime();
    let engine = RpmMacroEngine::from_runtime(&executor, &[], &runtime);

    assert_eq!(
        engine
            .expand(
                "%{lua:print(rpm.glob('/data/*')[1]); \
                 print(require('demo').value); \
                 local f=rpm.open('/data/value'); print(f:read()); f:close()}"
            )
            .expect("target-root Lua macro"),
        "/data/value\n42\npayload\n"
    );
}

#[test]
fn load_builtin_reads_structured_macro_records_from_the_target_root() {
    let root = tempfile::tempdir().expect("target root");
    std::fs::write(
        root.path().join("macros.demo"),
        "# ignored\n%loaded target\n%parameter() value=%1\n",
    )
    .expect("macro file");
    let executor = ScriptletExecutor::new(root.path(), "demo", "1", PackageFormat::Rpm);
    let runtime = runtime();
    let engine = RpmMacroEngine::from_runtime(&executor, &[], &runtime);

    assert_eq!(
        engine
            .expand("%{load:/macros.demo}%{loaded}:%parameter ok")
            .expect("load target macro file"),
        "target:value=ok"
    );
}
