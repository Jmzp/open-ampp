import { useEffect, useRef } from "react";

/** Ejecuta `fn` ahora y cada `ms` milisegundos mientras el componente esté montado. */
export function usePolling(fn: () => void | Promise<unknown>, ms: number, deps: unknown[] = []) {
  const saved = useRef(fn);
  saved.current = fn;
  useEffect(() => {
    saved.current();
    const id = setInterval(() => saved.current(), ms);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ms, ...deps]);
}
