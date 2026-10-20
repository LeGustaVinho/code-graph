use diesel::prelude::*;

#[derive(Queryable, Selectable)]
#[diesel(table_name = customers)]
pub struct Customer {
    pub id: i32,
    pub name: String,
}

#[derive(Queryable, Insertable)]
#[diesel(table_name = invoices)]
pub struct Invoice {
    pub id: i32,
    pub customer_id: i32,
    pub amount: f64,
}

pub fn store(conn: &mut PgConnection, invoice: &Invoice) {
    diesel::insert_into(invoices::table).values(invoice).execute(conn).unwrap();
}

pub fn report() -> serde_json::Value {
    serde_json::json!({ "total": crate::services::api_base() })
}
