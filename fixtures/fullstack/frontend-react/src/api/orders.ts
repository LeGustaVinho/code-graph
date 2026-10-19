import axios from "axios";
import { API_BASE } from "../config";

export interface OrderDto {
  id: string;
  status: string;
}

/** Loads all orders from the backend. */
export async function getOrders(): Promise<OrderDto[]> {
  const res = await fetch("/api/orders");
  if (!res.ok) {
    throw new Error("Failed to load orders");
  }
  return res.json();
}

export async function cancelOrder(id: string): Promise<void> {
  await axios.post(`/api/orders/${id}/cancel`);
}

export async function getProfile(): Promise<unknown> {
  return axios.get(`${API_BASE}/profile`);
}
