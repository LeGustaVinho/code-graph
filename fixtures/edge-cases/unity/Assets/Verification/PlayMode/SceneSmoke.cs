// PlayMode test assembly found through its runner references (GUID form), in a folder whose
// name says nothing about tests.
using System.Collections;
using NUnit.Framework;
using UnityEngine.TestTools;

namespace Game.Verification
{
    public class SceneSmoke : IPrebuildSetup, IPostBuildCleanup
    {
        public void Setup()
        {
        }

        public void Cleanup()
        {
        }

        [UnitySetUp]
        public IEnumerator Prepare()
        {
            yield return null;
        }

        [UnityTest]
        public IEnumerator Boots()
        {
            yield return null;
        }
    }

    public class SmokeSettings : UnityEngine.ScriptableObject
    {
        // Same name, but a ScriptableObject is not a runner hook.
        public void Setup()
        {
        }
    }
}
