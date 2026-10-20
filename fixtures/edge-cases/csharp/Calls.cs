using System;
using System.Collections.Generic;

namespace Edge.Calls
{
    public interface ILogger2
    {
        void Log(string message);
    }

    public class Svc
    {
        public void Activate() { }
    }

    public class Order
    {
        public string Name { get; set; }
        public void Ship() { }
    }

    public class Caller
    {
        private ILogger2 _logger;
        private string Name;
        public event EventHandler Changed;

        // Decoy: an unqualified `Log` in this class must not capture `_logger?.Log(...)`.
        public void Log(string message) { }

        private Svc FindUser(int id) => new Svc();

        private (int, int) Compute() => (1, 2);

        public void Run(List<Order> orders)
        {
            _logger?.Log("x");
            FindUser(1)?.Activate();
            Changed?.Invoke(this, EventArgs.Empty);
            var (a, b) = Compute();
            var label = nameof(Process);
            var order = new Order { Name = label };
            foreach (Order o in orders)
            {
                o.Ship();
            }
        }

        private void Process() { }
    }

    public class Params
    {
        public void Write(string format) { }
        public void Write(string format, params object[] args) { }
        public void Read([Optional(Name = "q")] string query) { }

        public void Use()
        {
            Write("{0} {1}", 1, 2);
        }
    }

    public class OptionalAttribute : Attribute
    {
        public string Name { get; set; }
    }

    public class Widget
    {
        static Widget() { }
        public Widget(int size) { }
    }

    public class OnlyStatic
    {
        static OnlyStatic() { }
    }

    public class Factory
    {
        public Widget Make() => new Widget(3);
        public OnlyStatic MakeStatic() => new OnlyStatic();
    }

    public record struct Point(int X, int Y);

    public class Props
    {
        public string Title => "edge-title";
    }
}

namespace Edge.Outer
{
    namespace Inner
    {
        public class Deep
        {
        }
    }
}
