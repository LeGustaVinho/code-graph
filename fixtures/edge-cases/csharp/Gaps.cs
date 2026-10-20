using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using static Edge.Gaps.MathUtil;
using Calc = Edge.Gaps.MathUtil;

namespace Edge.Gaps
{
    public class Entity
    {
        public void Save() { }
    }

    public class Order : Entity
    {
        public decimal Total { get; set; }
        public bool IsPaid() => Total > 0;
        public void Ship() { }
    }

    public class Dto { }

    public interface IServiceCollection { }

    public static class OrderExtensions
    {
        public static Dto ToDto(this Order order) => new Dto();
        public static IServiceCollection AddRepos(this IServiceCollection services) => services;
    }

    public enum Color { Red, Green }

    public record Person(string Name, int Age);

    public static class MathUtil
    {
        public static int Clamp2(int v) => v;
    }

    public class Repo<T> where T : Entity
    {
        public Order Find(int id) => new Order();
        public Task<Order> LoadAsync() => Task.FromResult(new Order());
        public void Touch(T item) { item.Save(); }
        public void Each<U>(U u) where U : Entity { u.Save(); }
    }

    public class Usage
    {
        private Repo<Order> _repo;

        public async Task Run(List<Order> orders, IServiceCollection services, object y, Person p)
        {
            var o = _repo.Find(1);
            o.Ship();
            var dto = o.ToDto();
            services.AddRepos();
            foreach (var each in orders)
            {
                each.Ship();
            }
            orders[0].Save();
            var paid = orders.Where(x => x.IsPaid()).ToList();
            var cast = y as Order;
            cast.Ship();
            var loaded = await _repo.LoadAsync();
            loaded.Ship();
            var total = o.Total;
            var red = Color.Red;
            var name = p.Name;
            Order created = new();
            var t = typeof(Dto);
            Clamp2(1);
            Calc.Clamp2(2);
        }
    }

    public class Base2
    {
        public Base2(int a) { }
    }

    public class Derived2 : Base2
    {
        public Derived2() : base(1) { }
        public Derived2(string s) : this() { }
    }

    public class Primary(int x) : Base2(x)
    {
    }

    public class MarkerAttribute : Attribute { }

    [Marker]
    public class Tagged { }

    public struct Money
    {
        public static Money Add(Money a, Money b) => a;
        public static Money operator +(Money a, Money b) { return Add(a, b); }
    }

    public class Events
    {
        private void Hook() { }
        public event Action Changed { add { Hook(); } remove { } }
    }

    public delegate void Handler(int x);

    public class Locals
    {
        private int Helper() => 0;

        public int Compute()
        {
            int Helper(int a) => a * 2;
            return Helper(3);
        }
    }

    public class Animal
    {
        public virtual void Speak() { }
    }

    public class Dog : Animal
    {
        public override void Speak() { }
    }

    public class Zoo
    {
        public void Play(Animal a) { a.Speak(); }
    }

    public interface IStore
    {
        void Persist();
    }

    public class Store : IStore
    {
        void IStore.Persist() { }
    }

    public class StoreUser
    {
        public void Use(IStore s) { s.Persist(); }
    }
}
