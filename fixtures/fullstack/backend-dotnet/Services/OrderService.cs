using Microsoft.EntityFrameworkCore;
using Shop.Data;
using Shop.Exceptions;
using Shop.Models;

namespace Shop.Services
{
    public class OrderService : IOrderService
    {
        private const int MaxItems = 50;
        private readonly AppDbContext _db;
        private readonly IClock _clock;
        private int _placedCount;

        public OrderService(AppDbContext db, IClock clock)
        {
            _db = db;
            _clock = clock;
        }

        public async Task<List<Order>> ListOrdersAsync()
        {
            return await _db.Orders.Include(o => o.Items).ToListAsync();
        }

        public async Task<Order> PlaceOrderAsync(Order order, int priority)
        {
            if (order.Items.Count == 0 || order.Items.Count > MaxItems)
            {
                throw new ArgumentException("Order must contain between 1 and 50 items");
            }

            foreach (var item in order.Items)
            {
                if (item.Quantity <= 0 && priority > 1)
                {
                    item.Quantity = 1;
                }
                else if (item.Quantity > 100)
                {
                    item.Quantity = 100;
                }
            }

            order.CreatedAt = _clock.Now();
            _db.Orders.Add(order);
            _placedCount++;
            await _db.SaveChangesAsync();
            return order;
        }

        public async Task CancelOrderAsync(Guid id, string reason)
        {
            var order = await _db.Orders.FindAsync(id);
            if (order == null)
            {
                throw new OrderNotFoundException($"Order {id} not found");
            }
            order.Status = OrderStatus.Cancelled;
            Audit(order, reason);
            await _db.SaveChangesAsync();
        }

        private void Audit(Order order, string reason)
        {
            Console.WriteLine($"cancel {order.Id}: {reason}");
        }
    }
}
