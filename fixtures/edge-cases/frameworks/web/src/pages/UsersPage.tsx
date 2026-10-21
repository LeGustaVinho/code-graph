import { useContext, useState } from 'react';
import { listUsers } from '#app/lib/api';
import { AuthContext } from '#app/auth';

export function UsersPage() {
  const session = useContext(AuthContext);
  const [count, setCount] = useState(0);
  const reload = () => {
    setCount(count + 1);
    listUsers();
    session.refresh();
  };
  return (
    <button data-testid="refresh-users" onClick={reload}>
      Users {count}
    </button>
  );
}
