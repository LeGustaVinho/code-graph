using Debug = UnityEngine.Debug;
using Object = UnityEngine.Object;

namespace Edge.Aliases
{
    public class Logger3
    {
        // A using alias named like its own target must not recurse forever.
        public void Log(Object o) { Debug.Log(o); }
    }
}
