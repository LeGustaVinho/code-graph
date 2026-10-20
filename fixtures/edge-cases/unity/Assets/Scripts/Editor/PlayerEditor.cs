using UnityEditor;
using UnityEngine;

namespace Game.EditorTools
{
    [CustomEditor(typeof(Game.Player))]
    public class PlayerEditor : Editor
    {
        public override void OnInspectorGUI() { }
    }

    [CustomPropertyDrawer(typeof(Range2))]
    public class RangeDrawer : PropertyDrawer
    {
        public override float GetPropertyHeight(SerializedProperty property, GUIContent label) => 0f;
    }

    public class Range2 { }

    [InitializeOnLoad]
    public static class Boot
    {
        static Boot() { Setup(); }

        private static void Setup() { }
    }
}
