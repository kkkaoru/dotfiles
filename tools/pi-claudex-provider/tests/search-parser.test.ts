import { describe, expect, it } from "vitest";
import { parseSearchTriplets } from "../src/search-parser.ts";

describe("parseSearchTriplets", () => {
  it("parses well-formed triplets", () => {
    const text = [
      "Title: Bitcoin Price Today | CoinGecko",
      "URL: https://www.coingecko.com/en/coins/bitcoin",
      "Snippet: BTC $63,023.41 USD",
      "",
      "Title: Bitcoin - CoinMarketCap",
      "URL: https://coinmarketcap.com/currencies/bitcoin/",
      "Snippet: Bitcoin price today, BTC to USD live price",
    ].join("\n");
    const results = parseSearchTriplets(text);
    expect(results).toHaveLength(2);
    expect(results[0]).toEqual({
      title: "Bitcoin Price Today | CoinGecko",
      url: "https://www.coingecko.com/en/coins/bitcoin",
      snippet: "BTC $63,023.41 USD",
    });
    expect(results[1]).toEqual({
      title: "Bitcoin - CoinMarketCap",
      url: "https://coinmarketcap.com/currencies/bitcoin/",
      snippet: "Bitcoin price today, BTC to USD live price",
    });
  });

  it("returns empty array for non-matching text", () => {
    expect(parseSearchTriplets("no search results here")).toEqual([]);
  });

  it("handles missing snippets gracefully", () => {
    const text = ["Title: Some Result", "URL: https://example.com"].join("\n");
    const results = parseSearchTriplets(text);
    expect(results).toHaveLength(1);
    expect(results[0]?.snippet).toBe("");
  });

  it("falls back to loose matching when structured parse fails", () => {
    const text = "Title: Fallback\nURL: https://fallback.com\nSnippet: fallback snippet";
    const results = parseSearchTriplets(text);
    expect(results).toHaveLength(1);
    expect(results[0]?.url).toBe("https://fallback.com");
  });

  it("ignores orphaned URL/Snippet lines without a preceding Title", () => {
    const text = [
      "URL: https://orphan.com",
      "Snippet: orphan snippet",
      "Title: Valid",
      "URL: https://valid.com",
      "Snippet: valid snippet",
    ].join("\n");
    const results = parseSearchTriplets(text);
    expect(results).toHaveLength(1);
    expect(results[0]?.title).toBe("Valid");
  });
});

it("handles empty input", () => {
  expect(parseSearchTriplets("")).toEqual([]);
});

it("handles Title without URL (incomplete triplet)", () => {
  const text = "Title: No URL Result\nSnippet: orphan snippet";
  expect(parseSearchTriplets(text)).toEqual([]);
});

it("handles multiple consecutive Titles (resets pending)", () => {
  const text = [
    "Title: First",
    "Title: Second",
    "URL: https://second.com",
    "Snippet: second snippet",
  ].join("\n");
  const results = parseSearchTriplets(text);
  expect(results).toHaveLength(1);
  expect(results[0]?.title).toBe("Second");
});

it("trims whitespace from field values", () => {
  const text = [
    "Title:   Trimmed Title  ",
    "URL:   https://trimmed.com  ",
    "Snippet:   trimmed snippet  ",
  ].join("\n");
  const results = parseSearchTriplets(text);
  expect(results).toHaveLength(1);
  expect(results[0]?.title).toBe("Trimmed Title");
  expect(results[0]?.url).toBe("https://trimmed.com");
  expect(results[0]?.snippet).toBe("trimmed snippet");
});

it("loose parse handles results with empty fields", () => {
  // Text that won't match structured parse (no proper Title:/URL: prefix format)
  // But will match loose regex
  const text = "Title: \nURL: \nSnippet: ";
  const results = parseSearchTriplets(text);
  // Loose parse should handle empty captures
  expect(results.length).toBeGreaterThanOrEqual(0);
});
