import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "./dialog";

describe("Dialog", () => {
  it("opens with an accessible title", async () => {
    render(
      <Dialog defaultOpen>
        <DialogTrigger>abrir</DialogTrigger>
        <DialogContent>
          <DialogTitle>Configurações</DialogTitle>
        </DialogContent>
      </Dialog>,
    );
    expect(screen.getByText("Configurações")).toBeInTheDocument();
  });
});
