pub fn seeded_service() -> shop_api::services::OrderService {
    shop_api::services::OrderService::new(Box::new(shop_api::repo::MemoryRepo::default()))
}
