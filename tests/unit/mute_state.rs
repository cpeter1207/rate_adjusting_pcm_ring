use super::*;

fn interval(generation: u64, start: u64, end: u64) -> Interval {
    Interval {
        generation,
        span: Span { start, end },
    }
}

fn position(generation: u64, low: u64, high: u64) -> Option<Position> {
    Some(Position {
        generation,
        low,
        high,
    })
}

#[test]
fn preparation_reports_mailbox_and_history_allocation_failures() {
    assert!(
        crate::tests::fail_allocation_at(QUEUE * size_of::<Interval>(), 1, Mailbox::new,).is_err()
    );
    assert!(
        crate::tests::fail_allocation_at(2 * size_of::<Interval>(), 1, || History::new(2),)
            .is_err()
    );
}

#[test]
fn mailbox_clips_empty_suffix_without_spending_interval_credit() {
    let mailbox = Mailbox::new().unwrap();
    mailbox.publish(1, &[Span { start: 10, end: 20 }], 10);
    assert_eq!(mailbox.free(), QUEUE);
    assert!(mailbox.pop().is_none());
    mailbox.publish(2, &[Span { start: 5, end: 20 }], 10);
    let clipped = mailbox.pop().unwrap();
    assert_eq!(clipped.generation, 2);
    assert_eq!(clipped.span, Span { start: 5, end: 10 });
    mailbox.reset();
    assert_eq!(mailbox.free(), QUEUE);
}

#[test]
fn bounded_history_merges_only_overlapping_same_generation_intervals() {
    let mut history = History::new(4).unwrap();
    for record in [
        interval(1, 10, 20),
        interval(2, 100, 200),
        interval(1, 30, 40),
        interval(1, 0, 5),
    ] {
        history.insert(record).unwrap();
    }
    assert!(history.insert(interval(3, 50, 60)).is_err());
    history.insert(interval(1, 15, 35)).unwrap();
    assert_eq!(history.0.len(), 3);
    assert_eq!(history.0[1].span, Span { start: 10, end: 40 });
    assert!(!history.muted(None));
    assert!(!history.muted(position(0, 0, 0)));
    assert!(history.muted(position(1, 15, 16)));
    assert!(!history.muted(position(1, 40, 41)));
    assert!(!history.muted(position(3, 100, 101)));
    history.retire(None);
    history.retire(position(1, 0, 0));
    assert_eq!(history.0.len(), 3);
    history.retire(position(2, 200, 200));
    assert!(history.0.is_empty());
    history.clear();
}
