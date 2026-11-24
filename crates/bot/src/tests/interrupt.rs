use super::*;

#[test]
fn a_registered_turn_can_be_stopped() {
    let interrupts = TurnInterrupts::default();
    let (token, _guard) = interrupts.register(7);
    assert!(!token.is_cancelled());
    assert!(interrupts.cancel(7));
    assert!(token.is_cancelled());
}

#[test]
fn stopping_a_channel_with_no_turn_reports_nothing_to_stop() {
    let interrupts = TurnInterrupts::default();
    assert!(!interrupts.cancel(7));
}

#[test]
fn stopping_twice_stays_true_and_changes_nothing() {
    let interrupts = TurnInterrupts::default();
    let (token, _guard) = interrupts.register(7);
    assert!(interrupts.cancel(7));
    assert!(interrupts.cancel(7));
    assert!(token.is_cancelled());
}

#[test]
fn only_the_named_channel_is_stopped() {
    let interrupts = TurnInterrupts::default();
    let (mine, _mine_guard) = interrupts.register(7);
    let (theirs, _theirs_guard) = interrupts.register(8);
    assert!(interrupts.cancel(7));
    assert!(mine.is_cancelled());
    assert!(!theirs.is_cancelled());
}

#[test]
fn a_finished_turn_deregisters_itself() {
    let interrupts = TurnInterrupts::default();
    {
        let (_token, _guard) = interrupts.register(7);
        assert!(interrupts.is_running(7));
    }
    assert!(!interrupts.is_running(7));
    assert!(!interrupts.cancel(7));
}

#[test]
fn a_later_turn_survives_the_earlier_guard_dropping() {
    let interrupts = TurnInterrupts::default();
    let (first, first_guard) = interrupts.register(7);
    let (second, _second_guard) = interrupts.register(7);
    drop(first_guard);

    assert!(interrupts.is_running(7));
    assert!(interrupts.cancel(7));
    assert!(second.is_cancelled());
    assert!(!first.is_cancelled());
}

#[test]
fn a_token_registered_again_after_being_stopped_starts_clean() {
    let interrupts = TurnInterrupts::default();
    {
        let (_token, _guard) = interrupts.register(7);
        interrupts.cancel(7);
    }
    let (fresh, _guard) = interrupts.register(7);
    assert!(!fresh.is_cancelled());
}
