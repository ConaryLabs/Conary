// conary-test/src/container/exec_supervisor.rs

use anyhow::{Context, Result};

const PID_MARKER: &str = "__CONARY_TEST_SYNC_SUPERVISOR__=";

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SupervisedExec {
    pub(super) supervisor_pid: u64,
    pub(super) child_process_group: u64,
}

const SUPERVISOR_SCRIPT: &str = r#"
child_pid=
kill_bin=
terminate_child() {
    trap - TERM INT HUP
    if [ -n "${child_pid}" ]; then
        "${kill_bin}" -TERM -- "-${child_pid}" 2>/dev/null || true
        sleep 0.2
        "${kill_bin}" -KILL -- "-${child_pid}" 2>/dev/null || true
        wait "${child_pid}" 2>/dev/null || true
    fi
    exit 124
}
trap terminate_child TERM INT HUP
if ! command -v setsid >/dev/null 2>&1; then
    echo "conary-test synchronous exec requires setsid" >&2
    exit 125
fi
for candidate in /bin/kill /usr/bin/kill; do
    if [ -x "${candidate}" ]; then
        kill_bin=${candidate}
        break
    fi
done
if [ -z "${kill_bin}" ]; then
    echo "conary-test synchronous exec requires an external kill executable" >&2
    exit 125
fi
setsid "$@" &
child_pid=$!
printf '__CONARY_TEST_SYNC_SUPERVISOR__=%s:%s\n' "$$" "${child_pid}"
wait "${child_pid}"
exit $?
"#;

pub(super) fn supervised_command(command: &[&str]) -> Vec<String> {
    let mut supervised = vec![
        "sh".to_string(),
        "-c".to_string(),
        SUPERVISOR_SCRIPT.to_string(),
        "conary-test-sync-exec".to_string(),
    ];
    supervised.extend(command.iter().map(|argument| (*argument).to_string()));
    supervised
}

pub(super) fn termination_command(supervised_exec: &SupervisedExec) -> Vec<String> {
    vec![
        "kill".to_string(),
        "-TERM".to_string(),
        "--".to_string(),
        supervised_exec.supervisor_pid.to_string(),
    ]
}

pub(super) fn take_supervised_exec(stdout: &mut String) -> Result<SupervisedExec> {
    let marker_start = stdout
        .find(PID_MARKER)
        .context("synchronous exec did not report its supervisor PID")?;
    let value_start = marker_start + PID_MARKER.len();
    let value_end = stdout[value_start..]
        .find('\n')
        .map_or(stdout.len(), |offset| value_start + offset);
    let (supervisor_pid, child_process_group) = stdout[value_start..value_end]
        .split_once(':')
        .context("synchronous exec did not report its child process group")?;
    let supervisor_pid = supervisor_pid
        .parse::<u64>()
        .context("synchronous exec reported an invalid supervisor PID")?;
    let child_process_group = child_process_group
        .parse::<u64>()
        .context("synchronous exec reported an invalid child process group")?;
    let remove_end = value_end.saturating_add(usize::from(value_end < stdout.len()));
    stdout.replace_range(marker_start..remove_end, "");
    Ok(SupervisedExec {
        supervisor_pid,
        child_process_group,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronous_exec_is_owned_by_a_signal_reaping_supervisor() {
        let command = supervised_command(&["sh", "-c", "sleep 300 & wait"]);
        assert_eq!(command[0], "sh");
        assert_eq!(command[3], "conary-test-sync-exec");
        assert_eq!(&command[4..], &["sh", "-c", "sleep 300 & wait"]);
        for required in [
            "trap terminate_child TERM INT HUP",
            "setsid \"$@\" &",
            "for candidate in /bin/kill /usr/bin/kill",
            "\"${kill_bin}\" -TERM -- \"-${child_pid}\"",
            "\"${kill_bin}\" -KILL -- \"-${child_pid}\"",
            "wait \"${child_pid}\"",
        ] {
            assert!(
                command[2].contains(required),
                "supervisor must enforce {required}"
            );
        }
    }

    #[test]
    fn supervisor_pid_is_parsed_and_removed_without_losing_command_output() {
        let mut stdout = format!("child output\n{PID_MARKER}742:743\nmore output\n");
        assert_eq!(
            take_supervised_exec(&mut stdout).unwrap(),
            SupervisedExec {
                supervisor_pid: 742,
                child_process_group: 743,
            }
        );
        assert_eq!(stdout, "child output\nmore output\n");
    }

    #[test]
    fn timeout_cleanup_signals_the_reaping_supervisor() {
        let command = termination_command(&SupervisedExec {
            supervisor_pid: 742,
            child_process_group: 743,
        });
        assert_eq!(command, ["kill", "-TERM", "--", "742"]);
        assert!(
            !command.iter().any(|argument| argument == "743"),
            "only the supervisor may terminate and reap its child process group"
        );
    }

    #[test]
    fn missing_or_malformed_supervisor_pid_fails_closed() {
        let mut missing = "ordinary output\n".to_string();
        assert!(take_supervised_exec(&mut missing).is_err());

        let mut malformed = format!("{PID_MARKER}not-a-pid\n");
        assert!(take_supervised_exec(&mut malformed).is_err());

        let mut malformed_group = format!("{PID_MARKER}742:not-a-pid\n");
        assert!(take_supervised_exec(&mut malformed_group).is_err());
    }
}
