using UnityEditor;
using UnityEditor.Callbacks;

namespace Game.Build
{
    public static class PostBuild
    {
        [PostProcessBuild]
        public static void OnPostprocessBuild(BuildTarget target, string path) { }
    }
}
