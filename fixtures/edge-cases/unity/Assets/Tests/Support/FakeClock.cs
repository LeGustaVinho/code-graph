// A support assembly under `Tests/` that references no test framework: not a test assembly.
namespace Game.TestSupport
{
    public class FakeClock
    {
        public float Now => 0f;
    }
}
