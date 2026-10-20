import { memo } from 'react';
import { withRouter } from 'react-router';

interface Props {
  label: string;
}

export const Row = memo((p: Props) => <div>{p.label}</div>);

function Page() {
  return <Row label="x" />;
}

export default withRouter(Page);
