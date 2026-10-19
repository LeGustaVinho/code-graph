namespace Shop.Handlers
{
    public interface IHandler
    {
        void Execute(string message);
    }

    public class EmailHandler : IHandler
    {
        public void Execute(string message) { }
    }

    public class SmsHandler : IHandler
    {
        public void Execute(string message) { }
    }

    public class PushHandler : IHandler
    {
        public void Execute(string message) { }
    }

    public class Dispatcher
    {
        private readonly List<IHandler> _handlers = new();

        public void Dispatch(IHandler handler, string message)
        {
            handler.Execute(message);
        }

        public void Broadcast(string message)
        {
            foreach (var h in _handlers)
            {
                Dispatch(h, message);
            }
        }
    }
}
