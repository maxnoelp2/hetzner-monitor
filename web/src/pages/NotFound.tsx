import { Link } from "react-router";
import { Empty } from "../ui/components";

export function NotFoundPage() {
  return (
    <Empty title="Page not found">
      <Link to="/alerts">Go to alerts</Link>
    </Empty>
  );
}
