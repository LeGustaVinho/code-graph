namespace Edge.Lib
{
    public class LibThing
    {
        public void Work() { }
    }
}

namespace Edge.Other
{
    public class LibThing
    {
        public void Work() { }
    }
}

namespace Edge.App
{
    public class Consumer
    {
        // Only the project-wide `global using Edge.Lib;` makes the right LibThing visible.
        public void Go(LibThing t) => t.Work();
    }
}
