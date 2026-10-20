use shop_api::repo::MemoryRepo;
use shop_api::services::OrderService;

#[test]
fn checkout_empty_order_fails() {
    let mut svc = OrderService::new(Box::new(MemoryRepo::default()));
    svc.open(1);
    assert!(svc.checkout(1).is_err());
}
