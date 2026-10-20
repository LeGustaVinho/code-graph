// No `using UnityEngine;` here: Player is a MonoBehaviour only through BaseBehaviour.
using System.Collections;
using Game.Core;

namespace Game
{
    public class Player : BaseBehaviour
    {
        private const string CoinsKey = "coins";

        [UnityEngine.ContextMenuItem("Reset", "ResetHealth")]
        public int health;

        void Update()
        {
            Move();
        }

        void Start()
        {
            StartCoroutine(nameof(Spawn));
            Invoke("Fire", 2f);
            var rb = GetComponent<Rigid>();
            rb.Push();
            UnityEngine.PlayerPrefs.SetInt(CoinsKey, 1);
            var level = UnityEngine.PlayerPrefs.GetInt("level", 0);
            transform.DOMove(1f).SetTarget(this);
            transform.Find("x").Warp();
            GetComponent<UnityEngine.UI.Button>().onClick.AddListener(Fire);
        }

        public void Review()
        {
#if UNITY_IOS
            AskIos();
#elif UNITY_ANDROID
            AskAndroid();
#endif
        }

        private void AskIos() { }
        private void AskAndroid() { }
        private void Move() { }
        private IEnumerator Spawn() { yield return null; }
        private void Fire() { }
        private void ResetHealth() { health = 100; }

        // Wired from a Button's OnClick in a scene: no code reference.
        public void OnButtonClicked() { }

        private void NeverUsed() { }
    }

    public class Rigid : UnityEngine.MonoBehaviour
    {
        public void Push() { }
    }

    // A project method sharing a name with an engine API must not capture engine calls.
    public class LifecycleObserver
    {
        public void AddListener(System.Action a) { }
    }

    [UnityEngine.RequireComponent(typeof(Rigid))]
    public class Mover : UnityEngine.MonoBehaviour
    {
        void Awake() { }
    }

    public class Clicker : UnityEngine.MonoBehaviour, UnityEngine.EventSystems.IPointerClickHandler
    {
        public void OnPointerClick(UnityEngine.EventSystems.PointerEventData e) { }
    }

    public class FancyButton : UnityEngine.UI.Button
    {
        protected override void Awake() { }
    }

    public class Disposer : System.IDisposable
    {
        // Named like a Unity message, but nothing here is a Unity object.
        public void Reset() { }
        public void Dispose() { }
    }

    [UnityEngine.CreateAssetMenu]
    public class GameConfig : UnityEngine.ScriptableObject
    {
        public int lives;
    }

    public class Net
    {
        public void Download(string url)
        {
            var a = UnityEngine.Networking.UnityWebRequest.Get(url);
            var b = UnityEngine.Networking.UnityWebRequestTexture.GetTexture("https://cdn.example.com/a.png");
        }
    }
}
