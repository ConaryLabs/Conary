// conary-core/src/recipe/kitchen/reproducibility_env/tests.rs

use super::*;
use std::path::Path;

#[test]
fn test_hermetic_command_validation_rejects_shell_env_mutation_forms() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        ("SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
        ("export SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
        ("unset SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
        (
            "/usr/bin/env SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        ("env -i make", "environment"),
        ("env -u SOURCE_DATE_EPOCH make", "SOURCE_DATE_EPOCH"),
        ("env --unset=RUSTFLAGS make", "RUSTFLAGS"),
        ("/usr/bin/env -u CFLAGS make", "CFLAGS"),
        ("env -iu SOURCE_DATE_EPOCH make", "environment"),
        ("env - make", "environment"),
        (
            "env -C /tmp SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "env --chdir=/tmp SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "env -a custom SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "env --argv0=custom SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "env --debug SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        ("env -- SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        (
            "env --block-signal SOURCE_DATE_EPOCH=999 make",
            "--block-signal",
        ),
        ("env -S 'SOURCE_DATE_EPOCH=999 make'", "-S"),
        (
            "env --split-string='SOURCE_DATE_EPOCH=999 make'",
            "split-string",
        ),
        (
            "env 'BASH_FUNC_make%%=() { SOURCE_DATE_EPOCH=999 make; }' ./build.sh",
            "BASH_FUNC_make%%",
        ),
        ("make SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
        ("gmake RUSTFLAGS+=bad", "RUSTFLAGS"),
        ("make MAKEFLAGS=SOURCE_DATE_EPOCH=999", "MAKEFLAGS"),
        ("MAKEFLAGS=SOURCE_DATE_EPOCH=999 make", "MAKEFLAGS"),
        ("MAKEFLAGS+=SOURCE_DATE_EPOCH=999 make", "MAKEFLAGS"),
        ("env GNUMAKEFLAGS=RUSTFLAGS=bad make", "GNUMAKEFLAGS"),
        ("make --eval 'export SOURCE_DATE_EPOCH=999'", "--eval"),
        ("MAKEFILES=evil.mk make", "MAKEFILES"),
        ("env MAKEFILES=evil.mk make", "MAKEFILES"),
        ("make -f evil.mk", "-f"),
        ("make --file=evil.mk", "--file"),
        ("MAKEFLAGS=--file=evil.mk make", "MAKEFLAGS"),
        ("make -rfevil.mk", "-rfevil.mk"),
        ("MAKEFLAGS=-rfevil.mk make", "MAKEFLAGS"),
        ("GNUMAKEFLAGS=-rfevil.mk make", "GNUMAKEFLAGS"),
        ("make -rEexport SOURCE_DATE_EPOCH=999", "-rEexport"),
        (
            "MAKEFLAGS='-rEexport SOURCE_DATE_EPOCH=999' make",
            "MAKEFLAGS",
        ),
        ("make --ev=export SOURCE_DATE_EPOCH=999", "--ev"),
        ("MAKEFLAGS='--ev=export CFLAGS=bad' make", "MAKEFLAGS"),
        ("make --fi=evil.mk", "--fi"),
        ("GNUMAKEFLAGS=--fi=evil.mk make", "GNUMAKEFLAGS"),
        ("make --mak=evil.mk", "--mak"),
        ("make -I evil", "-I"),
        ("MAKEFLAGS=-Ievil make", "MAKEFLAGS"),
        ("GNUMAKEFLAGS=--include-dir=evil make", "GNUMAKEFLAGS"),
        ("make --inc=evil", "--inc"),
        (
            "name=SOURCE_DATE_EPOCH; export $name=999; make",
            "shell expansion",
        ),
        ("export${IFS}SOURCE_DATE_EPOCH=999; make", "shell expansion"),
        ("ARGS=SOURCE_DATE_EPOCH=999; make $ARGS", "shell expansion"),
        ("OPT=--inc=evil; make $OPT", "shell expansion"),
        (
            "BAD=SOURCE_DATE_EPOCH=999; MAKEFLAGS=$BAD make",
            "MAKEFLAGS",
        ),
        ("env -u $KEY make", "shell expansion"),
        ("printf -v $name 999; make", "shell expansion"),
        (
            "export $(printf SOURCE_DATE_EPOCH)=999; make",
            "command substitution",
        ),
        (
            "export `printf SOURCE_DATE_EPOCH`=999; make",
            "command substitution",
        ),
        (
            "make $(printf SOURCE_DATE_EPOCH=999)",
            "command substitution",
        ),
        (
            "MAKEFLAGS=$(printf SOURCE_DATE_EPOCH=999) make",
            "command substitution",
        ),
        (
            "make `printf -- --include-dir=evil`",
            "command substitution",
        ),
        (
            "sh <(printf %s \"export SOURCE_DATE_EPOCH=999; make -s\")",
            "process substitution",
        ),
        (
            "bash <(printf %s \"export SOURCE_DATE_EPOCH=999; make -s\")",
            "process substitution",
        ),
        ("export SOURCE_DATE_EPOCH{,}=999; make", "shell expansion"),
        ("make SOURCE_DATE_EPOCH{,}=999", "shell expansion"),
        ("make --inc{,}=evil", "shell expansion"),
        ("export *; make", "shell expansion"),
        ("env SOURCE* make", "shell expansion"),
        ("make all *", "shell expansion"),
        ("make all --include*", "shell expansion"),
        ("export SOURCE_DATE_EPOCH\\=999; make", "shell expansion"),
        ("e\\xport SOURCE_DATE_EPOCH=777; make", "shell expansion"),
        ("ex\"\"port SOURCE_DATE_EPOCH=333; make", "shell expansion"),
        ("make SOURCE_DATE_EPOCH\\=999", "shell expansion"),
        ("ma\\ke SOURCE_DATE_EPOCH=888", "shell expansion"),
        ("ma\"\"ke SOURCE_DATE_EPOCH=777", "shell expansion"),
        ("env SOURCE_DATE_EPOCH\\=999 make", "shell expansion"),
        ("MAKEFLAGS=SOURCE_DATE_EPOCH\\=999 make", "MAKEFLAGS"),
        (
            "ARGS=x,SOURCE_DATE_EPOCH=999; IFS=,; env -a $ARGS make -s",
            "shell expansion",
        ),
        (
            "ARGS=x,SOURCE_DATE_EPOCH=999; IFS=,; env --argv0 $ARGS make -s",
            "shell expansion",
        ),
        (
            "ARGS=dir,SOURCE_DATE_EPOCH=999; IFS=,; env -C $ARGS make -s",
            "shell expansion",
        ),
        (
            "ARGS=x,env,SOURCE_DATE_EPOCH=999; IFS=,; exec -a $ARGS make -s",
            "shell expansion",
        ),
        (
            "ARGS=x,SOURCE_DATE_EPOCH=999; IFS=,; env --argv0=$ARGS make -s",
            "shell expansion",
        ),
        (
            "ARGS=dir,SOURCE_DATE_EPOCH=999; IFS=,; env --chdir=$ARGS make -s",
            "shell expansion",
        ),
        (
            "command env SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "command -p env SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        ("exec env -u SOURCE_DATE_EPOCH make", "SOURCE_DATE_EPOCH"),
        (
            "command export SOURCE_DATE_EPOCH=999; make",
            "SOURCE_DATE_EPOCH",
        ),
        ("command unset SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
        ("exec -c make", "environment"),
        ("readonly SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
        ("readonly SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
        (
            "command readonly SOURCE_DATE_EPOCH=999; make",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "readonly RUSTFLAGS=--remap-path-prefix=/src=/build/source-old; make",
            "RUSTFLAGS",
        ),
        ("time SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        ("! env SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        (
            "if SOURCE_DATE_EPOCH=999 make; then :; fi",
            "SOURCE_DATE_EPOCH",
        ),
        ("coproc SOURCE_DATE_EPOCH=999 make -s; wait", "coproc"),
        ("coproc make -s SOURCE_DATE_EPOCH=999; wait", "coproc"),
        ("sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("/bin/sh -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
        ("bash -ec 'SOURCE_DATE_EPOCH=999 make'", "-ec"),
        (
            "printf %s \"export SOURCE_DATE_EPOCH=999; make -s\" | sh",
            "stdin",
        ),
        (
            "echo \"export SOURCE_DATE_EPOCH=999; make -s\" | sh",
            "stdin",
        ),
        (
            "printf %s \"export SOURCE_DATE_EPOCH=999; make -s\" | bash",
            "stdin",
        ),
        ("sh < build.sh", "stdin"),
        ("bash -s < build.sh", "stdin"),
        ("sh build.sh", "script operand"),
        ("bash ./build.sh", "script operand"),
        ("busybox sh build.sh", "script operand"),
        (
            "printf %s \"export SOURCE_DATE_EPOCH=999; make -s\" > build.sh; sh build.sh",
            "script operand",
        ),
        ("ash -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("busybox sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("busybox ash -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
        ("env sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("env busybox ash -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        (
            "/usr/bin/env /bin/sh -c 'env -u SOURCE_DATE_EPOCH make'",
            "-c",
        ),
        ("env env SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        (
            "/usr/bin/env /usr/bin/env -u SOURCE_DATE_EPOCH make",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "builtin export SOURCE_DATE_EPOCH=999; make",
            "SOURCE_DATE_EPOCH",
        ),
        ("SOURCE_DATE_EPOCH+=999 env", "SOURCE_DATE_EPOCH"),
        ("SOURCE_DATE_EPOCH[0]=999 env", "SOURCE_DATE_EPOCH"),
        ("env SOURCE_DATE_EPOCH[0]=999 make", "SOURCE_DATE_EPOCH"),
        ("export SOURCE_DATE_EPOCH+=999; make", "SOURCE_DATE_EPOCH"),
        ("declare SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
        ("declare SOURCE_DATE_EPOCH+=999; make", "SOURCE_DATE_EPOCH"),
        ("declare -n ref=SOURCE_DATE_EPOCH; ref=999; make", "nameref"),
        (
            "declare -n ref=SOURCE_DATE_EPOCH; ref+=999; make",
            "nameref",
        ),
        ("typeset CFLAGS=bad; make", "CFLAGS"),
        ("typeset CFLAGS+=bad; make", "CFLAGS"),
        ("typeset -n ref=RUSTFLAGS; ref=bad; make", "nameref"),
        (
            "f(){ local SOURCE_DATE_EPOCH=999; make; }; f",
            "shell expansion",
        ),
        (
            "function f { local -n ref=RUSTFLAGS; ref=bad; make; }; f",
            "function",
        ),
        (
            "f(){ local SOURCE_DATE_EPOCH[0]=999; make; }; f",
            "shell expansion",
        ),
        ("readonly SOURCE_DATE_EPOCH+=999; make", "SOURCE_DATE_EPOCH"),
        (
            "declare SOURCE_DATE_EPOCH[0]=999; make",
            "SOURCE_DATE_EPOCH",
        ),
        ("readonly RUSTFLAGS[0]+=bad; make", "RUSTFLAGS"),
        (
            "read SOURCE_DATE_EPOCH <<EOF\n999\nEOF\nmake",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "read SOURCE_DATE_EPOCH[0] <<< 999; make",
            "SOURCE_DATE_EPOCH",
        ),
        ("read < file SOURCE_DATE_EPOCH; make", "read redirection"),
        ("mapfile SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
        ("readarray CFLAGS; make", "CFLAGS"),
        ("printf -v SOURCE_DATE_EPOCH 999; make", "SOURCE_DATE_EPOCH"),
        (
            "printf -v SOURCE_DATE_EPOCH[0] 999; make",
            "SOURCE_DATE_EPOCH",
        ),
        ("printf -v RUSTFLAGS[0] bad; make", "RUSTFLAGS"),
        ("let SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
        ("getopts ab SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
        ("eval 'SOURCE_DATE_EPOCH=999 make'", "eval"),
        ("source ./env-file; make", "source"),
        (". ./env-file; make", "."),
        (
            "export RUSTFLAGS=--remap-path-prefix=/src=/build/source-old; make",
            "RUSTFLAGS",
        ),
    ];

    for (command, key) in cases {
        let error =
            validate_command_local_reproducibility_env(&config, "make", command).unwrap_err();
        assert!(
            error.to_string().contains(key),
            "expected {key} rejection for {command}, got: {error}"
        );
    }
}

#[test]
fn test_env_wrapper_scanner_validates_nested_env_command() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        (
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "SOURCE_DATE_EPOCH=999 make".to_string(),
            ],
            "-c",
        ),
        (
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "env -u SOURCE_DATE_EPOCH make".to_string(),
            ],
            "-c",
        ),
        (
            vec![
                "env".to_string(),
                "SOURCE_DATE_EPOCH=999".to_string(),
                "make".to_string(),
            ],
            "SOURCE_DATE_EPOCH",
        ),
        (
            vec![
                "/usr/bin/env".to_string(),
                "-u".to_string(),
                "SOURCE_DATE_EPOCH".to_string(),
                "make".to_string(),
            ],
            "SOURCE_DATE_EPOCH",
        ),
    ];

    for (tokens, expected) in cases {
        let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_rejects_nested_shell_c_invocations() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        ("sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("/bin/sh -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
        ("bash -ec 'SOURCE_DATE_EPOCH=999 make'", "-ec"),
        ("dash -e -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("zsh -ce 'SOURCE_DATE_EPOCH=999 make'", "-ce"),
        ("ksh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("mksh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("ash -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("busybox sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
        ("busybox ash -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
    ];

    for (segment, expected) in cases {
        let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {segment}, got: {error}"
        );
    }
}

#[test]
fn test_env_wrapper_scanner_rejects_busybox_shell_applets() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let tokens = vec![
        "busybox".to_string(),
        "ash".to_string(),
        "-c".to_string(),
        "SOURCE_DATE_EPOCH=999 make".to_string(),
    ];

    let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

    assert!(error.to_string().contains("-c"));
}

#[test]
fn test_shell_env_scanner_rejects_control_word_hidden_env_mutations() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        ("time SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        ("time -p SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        ("! env SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        ("if SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        ("while SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        ("until SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
    ];

    for (segment, expected) in cases {
        let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {segment}, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_rejects_keyword_mode_set() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        ("set -k; make SOURCE_DATE_EPOCH=999", "-k"),
        ("set -ak; make RUSTFLAGS=bad", "-ak"),
        ("set -o keyword; make CFLAGS=bad", "keyword"),
    ];

    for (command, expected) in cases {
        let error = validate_shell_env_mutations(&config, "make", command).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {command}, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_rejects_alias_expansion_surfaces() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        (
            "shopt -s expand_aliases\nalias m='SOURCE_DATE_EPOCH=999 make'\nm",
            "shopt",
        ),
        (
            "shopt -s expand_aliases\nalias m='export SOURCE_DATE_EPOCH=999'\nm\nmake",
            "shopt",
        ),
        ("alias m='SOURCE_DATE_EPOCH=999 make'", "alias"),
        ("unalias m", "unalias"),
    ];

    for (command, expected) in cases {
        let error = validate_shell_env_mutations(&config, "make", command).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {command}, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_rejects_trap_env_bypasses() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        "trap 'export SOURCE_DATE_EPOCH=999' DEBUG; make",
        "trap 'SOURCE_DATE_EPOCH=999' DEBUG; make",
    ];

    for command in cases {
        let error = validate_shell_env_mutations(&config, "make", command).unwrap_err();

        assert!(
            error.to_string().contains("trap"),
            "expected trap rejection for {command}, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_fails_closed_on_unsupported_control_words() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [("time -v make", "-v"), ("for item in values", "for")];

    for (segment, expected) in cases {
        let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {segment}, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_rejects_readonly_controlled_vars() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        ("readonly SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
        ("readonly SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
        (
            "command readonly SOURCE_DATE_EPOCH=999",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "readonly RUSTFLAGS=--remap-path-prefix=/src=/build/source-old",
            "RUSTFLAGS",
        ),
    ];

    for (segment, expected) in cases {
        let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {segment}, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_rejects_append_assignments() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        ("SOURCE_DATE_EPOCH+=999 env", "SOURCE_DATE_EPOCH"),
        ("SOURCE_DATE_EPOCH[0]=999 env", "SOURCE_DATE_EPOCH"),
        ("export SOURCE_DATE_EPOCH+=999", "SOURCE_DATE_EPOCH"),
        ("declare SOURCE_DATE_EPOCH+=999", "SOURCE_DATE_EPOCH"),
        ("typeset CFLAGS+=bad", "CFLAGS"),
        ("readonly SOURCE_DATE_EPOCH+=999", "SOURCE_DATE_EPOCH"),
        ("readonly SOURCE_DATE_EPOCH[0]+=999", "SOURCE_DATE_EPOCH"),
    ];

    for (segment, expected) in cases {
        let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {segment}, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_rejects_assignment_capable_builtins() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        ("builtin export SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
        (
            "builtin printf -v SOURCE_DATE_EPOCH 999",
            "SOURCE_DATE_EPOCH",
        ),
        ("builtin -x export SOURCE_DATE_EPOCH=999", "-x"),
        ("declare SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
        ("declare -x SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
        ("declare -n ref=SOURCE_DATE_EPOCH", "nameref"),
        ("declare -gn ref=SOURCE_DATE_EPOCH", "nameref"),
        ("declare +n ref=SOURCE_DATE_EPOCH", "nameref"),
        ("declare -n ref=SOURCE_DATE_EPOCH; ref+=999", "nameref"),
        ("typeset CFLAGS=bad", "CFLAGS"),
        ("typeset -n ref=RUSTFLAGS", "nameref"),
        ("local SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
        ("local -n ref=RUSTFLAGS", "nameref"),
        ("local SOURCE_DATE_EPOCH[0]=999", "SOURCE_DATE_EPOCH"),
        ("function f { local SOURCE_DATE_EPOCH=999", "function"),
        ("declare SOURCE_DATE_EPOCH[0]=999", "SOURCE_DATE_EPOCH"),
        ("typeset CFLAGS[0]=bad", "CFLAGS"),
        ("read -r SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
        ("read -a SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
        ("read SOURCE_DATE_EPOCH[0] <<< 999", "SOURCE_DATE_EPOCH"),
        ("read < file SOURCE_DATE_EPOCH", "read redirection"),
        ("mapfile SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
        ("mapfile SOURCE_DATE_EPOCH[0]", "SOURCE_DATE_EPOCH"),
        ("readarray CFLAGS", "CFLAGS"),
        ("readarray CFLAGS[0]", "CFLAGS"),
        ("printf -v SOURCE_DATE_EPOCH 999", "SOURCE_DATE_EPOCH"),
        ("printf -v SOURCE_DATE_EPOCH[0] 999", "SOURCE_DATE_EPOCH"),
        ("printf -v RUSTFLAGS[0] bad", "RUSTFLAGS"),
        ("let SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
        ("let count=SOURCE_DATE_EPOCH+1", "SOURCE_DATE_EPOCH"),
        ("getopts ab SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
        ("eval 'SOURCE_DATE_EPOCH=999 make'", "eval"),
        ("source ./env-file", "source"),
        (". ./env-file", "."),
    ];

    for (segment, expected) in cases {
        let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {segment}, got: {error}"
        );
    }
}

#[test]
fn test_shell_env_scanner_peels_command_and_exec_wrappers() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let cases = [
        (
            "command env SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        (
            "command -p env SOURCE_DATE_EPOCH=999 make",
            "SOURCE_DATE_EPOCH",
        ),
        ("exec env -u SOURCE_DATE_EPOCH make", "SOURCE_DATE_EPOCH"),
        ("command export SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
        ("command unset SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
        ("exec -c make", "environment"),
    ];

    for (segment, expected) in cases {
        let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection for {segment}, got: {error}"
        );
    }
}

#[test]
fn test_env_wrapper_scanner_keeps_scanning_after_option_delimiter() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let tokens = vec![
        "--".to_string(),
        "SOURCE_DATE_EPOCH=999".to_string(),
        "make".to_string(),
    ];

    let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

    assert!(error.to_string().contains("SOURCE_DATE_EPOCH"));
}

#[test]
fn test_env_wrapper_scanner_rejects_unsupported_long_options() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    let tokens = vec![
        "--block-signal".to_string(),
        "SOURCE_DATE_EPOCH=999".to_string(),
        "make".to_string(),
    ];

    let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

    assert!(error.to_string().contains("--block-signal"));
}

#[test]
fn test_env_wrapper_scanner_rejects_split_string_options() {
    let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
    for (tokens, expected) in [
        (
            vec!["-S".to_string(), "SOURCE_DATE_EPOCH=999 make".to_string()],
            "-S",
        ),
        (
            vec!["--split-string=SOURCE_DATE_EPOCH=999 make".to_string()],
            "split-string",
        ),
    ] {
        let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

        assert!(
            error.to_string().contains(expected),
            "expected {expected} rejection, got: {error}"
        );
    }
}
