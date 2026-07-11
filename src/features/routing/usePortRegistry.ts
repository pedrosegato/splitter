import { createContext, useContext, useMemo, useRef } from "react";
import { createElement } from "react";
import type { ReactNode } from "react";
import type { PortRef } from "./resolveConnection";

type Registry = {
  register: (id: string, el: HTMLElement | null, ref?: PortRef) => void;
  get: (id: string) => HTMLElement | undefined;
  getRef: (id: string) => PortRef | null;
};

const Ctx = createContext<Registry | null>(null);

export function PortRegistryProvider({ children }: { children: ReactNode }) {
  const map = useRef(new Map<string, HTMLElement>());
  const refMap = useRef(new Map<string, PortRef>());

  const reg = useMemo<Registry>(
    () => ({
      register: (id, el, ref) => {
        if (el) {
          map.current.set(id, el);
          if (ref) refMap.current.set(id, ref);
        } else {
          map.current.delete(id);
          refMap.current.delete(id);
        }
      },
      get: (id) => map.current.get(id),
      getRef: (id) => refMap.current.get(id) ?? null,
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
