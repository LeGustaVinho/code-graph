namespace Edge.First
{
    public class Alpha
    {
        public void Ping() { }
    }
}

namespace Edge.Second
{
    public class Beta
    {
        // `Gamma` exists in Edge.Second and Edge.Third (Gammas.cs): only the namespace of
        // this second block disambiguates it.
        public void Go(Gamma g) => g.Pong();
    }
}
