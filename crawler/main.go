package main

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"

	"log"
	"net/url"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gocolly/colly/v2"
)

type Document struct {
	ID      string   `json:"id"`
	URL     string   `json:"url"`
	Title   string   `json:"title"`
	Content string   `json:"content"`
	Links   []string `json:"links"`
}

func main() {
	const maxPages = 30
	seedURL := "https://en.wikipedia.org/wiki/Search_engine"

	docs := make([]Document, 0, maxPages)
	var docsMu sync.Mutex

	var visitedCount int32
	var visitedURLs sync.Map

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	c := colly.NewCollector(
		colly.MaxDepth(2),
		colly.AllowedDomains("en.wikipedia.org"),
		colly.Async(true),
	)

	_ = c.Limit(&colly.LimitRule{
		DomainGlob:  "*",
		Parallelism: 4, // Increased slightly for higher throughput
		Delay:       200 * time.Millisecond,
	})

	c.OnRequest(func(r *colly.Request) {
		// Stop processing if context is canceled or page cap reached
		if ctx.Err() != nil || atomic.LoadInt32(&visitedCount) >= maxPages {
			r.Abort()
			return
		}

		cleanURL := normalizeURL(r.URL.String())
		if _, loaded := visitedURLs.LoadOrStore(cleanURL, true); loaded {
			r.Abort()
		}
	})

	c.OnHTML("html", func(e *colly.HTMLElement) {
		// Check atomic count before doing heavy parsing
		if atomic.LoadInt32(&visitedCount) >= maxPages {
			cancel()
			return
		}

		// Increment atomically
		currentCount := atomic.AddInt32(&visitedCount, 1)
		if currentCount > maxPages {
			cancel()
			return
		}

		pageURL := e.Request.URL.String()
		title := strings.TrimSpace(e.ChildText("title"))

		// Fast text extraction using Builder
		var builder strings.Builder
		e.ForEach("p", func(_ int, el *colly.HTMLElement) {
			text := strings.TrimSpace(el.Text)
			if text != "" {
				builder.WriteString(text)
				builder.WriteString(" ")
			}
		})
		content := strings.Join(strings.Fields(builder.String()), " ")

		// Collect outward links
		links := make([]string, 0, 20)
		e.ForEach("a[href]", func(_ int, el *colly.HTMLElement) {
			link := el.Request.AbsoluteURL(el.Attr("href"))
			if link != "" && !strings.Contains(link, "#") {
				links = append(links, link)
			}
		})

		doc := Document{
			ID:      hashURL(pageURL),
			URL:     pageURL,
			Title:   title,
			Content: content,
			Links:   links,
		}

		docsMu.Lock()
		docs = append(docs, doc)
		fmt.Printf("[%d/%d] Scraped: %s\n", currentCount, maxPages, pageURL)
		docsMu.Unlock()

		// Trigger limit reached shutdown instantly
		if currentCount == maxPages {
			cancel()
			return
		}

		// Queue next links
		for _, l := range links {
			if ctx.Err() != nil {
				break
			}
			_ = e.Request.Visit(l)
		}
	})

	c.OnError(func(r *colly.Response, err error) {
		if err.Error() != "Request Canceled" {
			log.Printf("Error requesting %s: %v", r.Request.URL, err)
		}
	})

	fmt.Printf("Starting crawl at: %s\n", seedURL)
	_ = c.Visit(seedURL)
	c.Wait()

	saveJSON("documents.json", docs)
}

func normalizeURL(rawURL string) string {
	u, err := url.Parse(rawURL)
	if err != nil {
		return rawURL
	}
	u.Fragment = "" // Strip anchor tags (#section) to avoid scraping duplicates
	return u.String()
}

func hashURL(url string) string {
	h := sha256.Sum256([]byte(url))
	return hex.EncodeToString(h[:6])
}

func saveJSON(filename string, docs []Document) {
	file, err := json.MarshalIndent(docs, "", "  ")
	if err != nil {
		log.Fatalf("Failed to serialize JSON: %v", err)
	}

	if err := os.WriteFile(filename, file, 0644); err != nil {
		log.Fatalf("Failed to write to file: %v", err)
	}

	fmt.Printf("\nDone! Saved %d documents to %s\n", len(docs), filename)
}
