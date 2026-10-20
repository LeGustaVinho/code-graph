mod common;

#[test]
fn opens_orders() {
    let mut svc = common::seeded_service();
    svc.open(2);
}
