//! Preserve the Broker's held-J refusal at the Runner entry boundary.

use std::io::Write;

pub(super) fn child_pid(
    root: &str,
    reply: &str,
    diagnostic: &mut impl Write,
) -> Result<i32, String> {
    reply
        .strip_prefix(&format!("held-joined {root} "))
        .and_then(|s| s.strip_suffix('\n'))
        .ok_or_else(|| {
            // Keep the whole reply, including an unclassified/malformed one.
            // Debug escaping makes the record one line without losing bytes.
            let error = format!("v30 held J refused: root={root}; broker reply={reply:?}");
            // Diagnostic failure must not replace the original refusal.
            let _ = writeln!(diagnostic, "OULIPOLY_KERNEL_V30_HELD_J_REFUSED={error}");
            error
        })?
        .parse()
        .map_err(|_| "v30 held child PID invalid".to_owned())
}

#[cfg(test)]
mod tests {
    use super::child_pid;

    #[test]
    fn refused_j_retains_broker_cause_in_error_and_record() {
        let reply = "error root join gate already held\n";
        let mut record = Vec::new();
        let error = child_pid("root-a", reply, &mut record).unwrap_err();
        assert_eq!(
            error,
            "v30 held J refused: root=root-a; broker reply=\"error root join gate already held\\n\""
        );
        assert_eq!(
            String::from_utf8(record).unwrap(),
            format!("OULIPOLY_KERNEL_V30_HELD_J_REFUSED={error}\n")
        );
    }

    #[test]
    fn unclassified_replies_are_retained_without_inventing_a_cause() {
        for reply in [
            "",
            "held-joined root-b 123\n",
            "held-joined root-a 123",
            "unknown\n\"cause\"\tλ\n",
        ] {
            let mut record = Vec::new();
            let error = child_pid("root-a", reply, &mut record).unwrap_err();
            assert!(error.ends_with(&format!("broker reply={reply:?}")));
            let record = String::from_utf8(record).unwrap();
            assert_eq!(
                record,
                format!("OULIPOLY_KERNEL_V30_HELD_J_REFUSED={error}\n")
            );
            assert_eq!(record.lines().count(), 1);
        }
    }

    #[test]
    fn existing_pid_acceptance_and_invalid_pid_error_are_unchanged() {
        let mut record = Vec::new();
        for pid in [123, 0, -1] {
            assert_eq!(
                child_pid(
                    "root-a",
                    &format!("held-joined root-a {pid}\n"),
                    &mut record
                ),
                Ok(pid)
            );
        }
        assert_eq!(
            child_pid("root-a", "held-joined root-a invalid\n", &mut record),
            Err("v30 held child PID invalid".to_owned())
        );
        assert!(record.is_empty());
    }

    #[test]
    fn failed_record_write_does_not_mask_or_accept_refusal() {
        struct Unavailable;
        impl std::io::Write for Unavailable {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("diagnostic unavailable"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let error = child_pid(
            "root-a",
            "error exact root admission fenced\n",
            &mut Unavailable,
        )
        .unwrap_err();
        assert!(error.contains("error exact root admission fenced\\n"));
        assert!(!error.contains("diagnostic unavailable"));
    }
}
