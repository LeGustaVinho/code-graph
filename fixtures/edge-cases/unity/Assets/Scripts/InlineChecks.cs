// Assembly-CSharp, but it declares NUnit tests ("Enable playmode tests for all assemblies").
using NUnit.Framework;

namespace Game
{
    public class InlineChecks
    {
        [Test]
        public void CoinsStartPositive()
        {
            Assert.Greater(MockData.StartingCoins(), 0);
        }
    }
}
