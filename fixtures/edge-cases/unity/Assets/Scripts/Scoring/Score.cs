namespace Game.Scoring
{
    public class Score
    {
        public int Total { get; private set; }

        public void Add(int points)
        {
            Total += points;
        }
    }
}
