use super::*;

#[test]
fn request_terminate_rejects_a_pid_that_does_not_fit_pid_t() {
    let err = request_terminate(u32::MAX).unwrap_err();
    assert!(
        err.to_string().contains("does not fit in pid_t"),
        "error should explain the overflow, got: {err}"
    );
}
