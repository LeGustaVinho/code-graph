import { createBrowserRouter, RouterProvider, Routes, Route } from 'react-router-dom';
import { UsersPage } from '#app/pages/UsersPage';
import { UserDetail } from '#app/pages/UserDetail';
import { Settings } from '#app/pages/Settings';

const router = createBrowserRouter([
  { path: '/users', element: <UsersPage />, children: [{ path: ':id', element: <UserDetail /> }] },
]);

export function App() {
  return <RouterProvider router={router} />;
}

export function LegacyRoutes() {
  return (
    <Routes>
      <Route path="/account">
        <Route path="settings" element={<Settings />} />
      </Route>
    </Routes>
  );
}
