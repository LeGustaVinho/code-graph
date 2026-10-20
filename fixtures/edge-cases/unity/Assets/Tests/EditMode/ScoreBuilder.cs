// Helper inside the test assembly: test code even without test attributes.
using Game.Scoring;

namespace Game.Tests
{
    public static class ScoreBuilder
    {
        public static Score WithTotal(int total)
        {
            var s = new Score();
            s.Add(total);
            return s;
        }
    }
}
