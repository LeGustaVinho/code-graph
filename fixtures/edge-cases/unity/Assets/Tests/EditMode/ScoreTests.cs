using NUnit.Framework;
using Game.Scoring;

namespace Game.Tests
{
    [TestFixture]
    public class ScoreTests
    {
        private int bonus;

        [SetUp]
        public void Init()
        {
            bonus = 2;
        }

        [OneTimeTearDown]
        public void Done()
        {
        }

        [Test]
        public void AddAccumulates()
        {
            var score = new Score();
            score.Add(3);
            score.Add(bonus);
            Assert.AreEqual(5, score.Total);
        }

        [TestCase(1)]
        [TestCase(4)]
        public void AddOnce(int points)
        {
            var score = new Score();
            score.Add(points);
            Assert.AreEqual(points, score.Total);
        }
    }
}
