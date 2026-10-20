namespace Edge.Partials
{
    public partial class Big
    {
        public Big(int size) { }

        public void B() { }
    }

    public class BigUser
    {
        public Big Make() => new Big(3);
    }
}
