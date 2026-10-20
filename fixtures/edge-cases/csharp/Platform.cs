namespace Edge.Platform
{
    public class StoreReview
    {
        public void Ask()
        {
#if UNITY_IOS
            AskIos();
#elif UNITY_ANDROID
            AskAndroid();
#else
            AskEditor();
#endif
        }

        private void AskIos() { }
        private void AskAndroid() { }
        private void AskEditor() { }
    }
}
