import { Button } from "@/components/ui/button";

export function App() {
  return (
    <div className="bg-board h-full flex flex-col items-center justify-center gap-4">
      <h1 className="text-accent text-2xl font-semibold tracking-wide">
        Splitter
      </h1>
      <p className="text-ink-2 text-sm">Audio routing — UI loading…</p>
      <Button variant="outline">Open session</Button>
    </div>
  );
}
