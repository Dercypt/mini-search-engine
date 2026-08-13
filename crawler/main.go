package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log"
	"net/url"
	"os"
	"regexp"
	"strings"
	"sync"
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
	maxPages := 1000
	seedURL := "https://en.wikipedia.org/wiki/Search_engine"

	var docs []Document
	var mu sync.Mutex
	visitedCount := 0

	// Track enqueued/visited URLs across threads
	visitedURLs := make(map[string]bool)

	c := colly.NewCollector(
		colly.MaxDepth(4),
		colly.AllowedDomains("en.wikipedia.org"),
		colly.Async(true),
		colly.UserAgent("MiniSearchEngineBot/1.0 (+https://github.com/yourusername/mini-search-engine)"),
	)
	c.IgnoreRobotsTxt = false

	c.Limit(&colly.LimitRule{
		DomainGlob:  "*",
		Parallelism: 5,
		Delay:       100 * time.Millisecond,
	})

	disallowed := []*regexp.Regexp{
		regexp.MustCompile(`(?i)/w/index\.php`),
		regexp.MustCompile(`(?i)/wiki/(Special|Talk|User|Wikipedia|File|MediaWiki|Template|Template_talk|Help|Portal|Category):`),
		regexp.MustCompile(`(?i)\?(action|printable|useskin)=`),
	}

	// Mark seed URL as visited
	visitedURLs[seedURL] = true

	c.OnRequest(func(r *colly.Request) {
		mu.Lock()
		defer mu.Unlock()

		// Abort request if target count reached
		if visitedCount >= maxPages {
			r.Abort()
		}
	})

	c.OnHTML("html", func(e *colly.HTMLElement) {
		mu.Lock()
		if visitedCount >= maxPages {
			mu.Unlock()
			return
		}

		visitedCount++
		currentCount := visitedCount
		mu.Unlock()

		pageURL := normalizeURL(e.Request.URL.String())
		title := strings.TrimSpace(e.ChildText("title"))

		// Extract article body paragraphs and section headers
		var textPieces []string
		e.ForEach("div.mw-parser-output > p, div.mw-parser-output h2, div.mw-parser-output h3", func(_ int, el *colly.HTMLElement) {
			txt := strings.TrimSpace(el.Text)
			if txt != "" {
				textPieces = append(textPieces, txt)
			}
		})
		content := strings.Join(textPieces, " ")

		var links []string
		var linksToVisit []string

		e.ForEach("a[href]", func(_ int, el *colly.HTMLElement) {
			rawLink := el.Request.AbsoluteURL(el.Attr("href"))
			if rawLink == "" {
				return
			}

			cleanLink := normalizeURL(rawLink)

			// Fast regex check against noise paths
			for _, re := range disallowed {
				if re.MatchString(cleanLink) {
					return
				}
			}

			links = append(links, cleanLink)

			// Thread-safe deduplication before queueing
			mu.Lock()
			if !visitedURLs[cleanLink] && visitedCount < maxPages {
				visitedURLs[cleanLink] = true
				linksToVisit = append(linksToVisit, cleanLink)
			}
			mu.Unlock()
		})

		doc := Document{
			ID:      hashURL(pageURL),
			URL:     pageURL,
			Title:   title,
			Content: content,
			Links:   links,
		}

		mu.Lock()
		docs = append(docs, doc)
		fmt.Printf("[%d/%d] Scraped: %s\n", currentCount, maxPages, pageURL)

		// Instant termination when target is hit
		if currentCount == maxPages {
			saveJSON("documents.json", docs)
			mu.Unlock()
			os.Exit(0)
		}
		mu.Unlock()

		// Enqueue filtered, unvisited links
		for _, link := range linksToVisit {
			e.Request.Visit(link)
		}
	})

	c.OnError(func(r *colly.Response, err error) {
		log.Printf("Error requesting %s: %v", r.Request.URL, err)
	})

	fmt.Printf("Starting optimized crawl at: %s\n", seedURL)
	c.Visit(seedURL)
	c.Wait()

	saveJSON("documents.json", docs)
}

func normalizeURL(rawURL string) string {
	u, err := url.Parse(rawURL)
	if err != nil {
		return rawURL
	}
	u.Fragment = ""
	return u.String()
}

func hashURL(url string) string {
	h := sha256.New()
	h.Write([]byte(url))
	return hex.EncodeToString(h.Sum(nil))[:12]
}

func saveJSON(filename string, docs []Document) {
	file, err := json.MarshalIndent(docs, "", "  ")
	if err != nil {
		log.Fatalf("Failed to serialize JSON: %v", err)
	}
	err = os.WriteFile(filename, file, 0644)
	if err != nil {
		log.Fatalf("Failed to write to file: %v", err)
	}
	fmt.Printf("\nDone! Saved %d clean documents to %s\n", len(docs), filename)
}
