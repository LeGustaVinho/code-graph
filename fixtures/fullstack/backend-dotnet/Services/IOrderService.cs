using Shop.Models;

namespace Shop.Services
{
    public interface IOrderService
    {
        Task<List<Order>> ListOrdersAsync();
        Task<Order> PlaceOrderAsync(Order order, int priority);
        Task CancelOrderAsync(Guid id, string reason);
    }

    public interface IClock
    {
        DateTime Now();
    }

    public class SystemClock : IClock
    {
        public DateTime Now() => DateTime.UtcNow;
    }
}
