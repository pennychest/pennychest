import { useEffect, useState } from "react";

/** Whole seconds since `running` became true, and 0 while it's false. */
export function useElapsedSeconds(running: boolean): number {
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => {
      clearInterval(timer);
      setElapsed(0);
    };
  }, [running]);

  return running ? elapsed : 0;
}
