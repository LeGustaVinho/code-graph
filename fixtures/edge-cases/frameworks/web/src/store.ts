import { create } from 'zustand';

export const useCart = create((set) => ({
  items: [] as string[],
  add: (item: string) => set((s: { items: string[] }) => ({ items: [...s.items, item] })),
}));
