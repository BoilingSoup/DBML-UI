CREATE TABLE "accounts" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "tier" TEXT CHECK ("tier" IN ('free', 'pro', 'enterprise')) NOT NULL DEFAULT 'free',
  "state" TEXT CHECK ("state" IN ('active', 'suspended'))
);

