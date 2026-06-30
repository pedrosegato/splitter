import { createContext, useContext, useMemo, useRef } from "react";
import { createElement } from "react";
import type { ReactNode } from "react";

type Registry = {
  register: (id: string, el: HTMLElement | null) => void;
  get: (id: string) => HTMLElement | undefined;
};

const Ctx = createContext<Registry | null>(null);

export function PortRegistryProvider({ children }: { children: ReactNode }) {
  const map = useRef(new Map<string, HTMLElement>());

  const reg = useMemo<Registry>(
    () => ({
      register: (id, el) => {
        if (el) map.current.set(id, el);
        else map.current.delete(id);
      },
      get: (id) => map.current.get(id),
    }),
    [],
  );

  return createElement(Ctx.Provider, { value: reg }, children);
}

export function usePortRegistry() {
  const c = useContext(Ctx);
  if (!c) throw new Error("PortRegistry missing");
  return c;
}
