using Shop.Models;
using Shop.Services;
using Xunit;

namespace Shop.Tests
{
    public class OrderServiceTests
    {
        [Fact]
        public async Task PlaceOrder_RejectsEmptyOrders()
        {
            var service = new OrderService(null, new SystemClock());
            await Assert.ThrowsAsync<ArgumentException>(() => service.PlaceOrderAsync(new Order(), 1));
        }
    }
}
