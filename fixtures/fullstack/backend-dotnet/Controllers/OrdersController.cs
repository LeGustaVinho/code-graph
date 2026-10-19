using Microsoft.AspNetCore.Authorization;
using Microsoft.AspNetCore.Mvc;
using Shop.Exceptions;
using Shop.Models;
using Shop.Services;

namespace Shop.Controllers
{
    /// <summary>HTTP API for orders.</summary>
    [ApiController]
    [Route("api/[controller]")]
    [Authorize]
    public class OrdersController : ControllerBase
    {
        private readonly IOrderService _orderService;

        public OrdersController(IOrderService orderService)
        {
            _orderService = orderService;
        }

        [HttpGet]
        public async Task<IActionResult> GetOrders()
        {
            var orders = await _orderService.ListOrdersAsync();
            return Ok(orders);
        }

        [HttpPost("{id}/cancel")]
        public async Task<IActionResult> Cancel(Guid id)
        {
            try
            {
                await _orderService.CancelOrderAsync(id, "requested by customer");
                return NoContent();
            }
            catch (OrderNotFoundException ex)
            {
                return NotFound(ex.Message);
            }
        }

        [HttpPost]
        public async Task<IActionResult> Create([FromBody] Order order)
        {
            var created = await _orderService.PlaceOrderAsync(order, 1);
            return Ok(created);
        }
    }
}
