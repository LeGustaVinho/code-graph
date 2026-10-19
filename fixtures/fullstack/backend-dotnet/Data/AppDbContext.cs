using Microsoft.EntityFrameworkCore;
using Shop.Models;

namespace Shop.Data
{
    public class AppDbContext : DbContext
    {
        public DbSet<Order> Orders { get; set; }
        public DbSet<Customer> Customers { get; set; }
        public DbSet<OrderItem> OrderItems { get; set; }
    }
}
