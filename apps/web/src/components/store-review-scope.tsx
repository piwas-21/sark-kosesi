"use client";

import { createContext, useContext, type ReactNode } from "react";

const StoreReviewContext = createContext(false);

export function StoreReviewScope({ children }: { children: ReactNode }) {
  return <StoreReviewContext value={true}>{children}</StoreReviewContext>;
}

export function useStoreReview(): boolean {
  return useContext(StoreReviewContext);
}
