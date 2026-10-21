import { Session } from '#app/auth';

type DetailProps = { session: Session; id?: string };

export function UserDetail({ session }: DetailProps) {
  return <button onClick={() => session.logout()}>Log out</button>;
}
