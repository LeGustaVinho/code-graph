using System.ComponentModel.DataAnnotations;

namespace Shop.Models
{
    public enum OrderStatus { Pending, Paid, Cancelled }

    public class Order
    {
        [Key]
        public Guid Id { get; set; }
        public Guid CustomerId { get; set; }
        public Customer Customer { get; set; }
        public List<OrderItem> Items { get; set; } = new();
        public OrderStatus Status { get; set; }
        public DateTime CreatedAt { get; set; }
    }

    public class Customer
    {
        public Guid Id { get; set; }
        public string Email { get; set; }
        public ICollection<Order> Orders { get; set; }
    }

    public class OrderItem
    {
        public int Id { get; set; }
        public Guid OrderId { get; set; }
        public Order Order { get; set; }
        public string Sku { get; set; }
        public int Quantity { get; set; }
    }
}
