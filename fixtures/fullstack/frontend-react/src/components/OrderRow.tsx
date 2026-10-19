import React from "react";
import { OrderDto } from "../api/orders";

type Props = { order: OrderDto; onCancel: (id: string) => void };

export const OrderRow = ({ order, onCancel }: Props) => {
  const label = order.status === "cancelled" ? "Cancelled" : "Active";
  return (
    <li>
      {order.id} - {label}
      <button onClick={() => onCancel(order.id)}>Cancel</button>
    </li>
  );
};
