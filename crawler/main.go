package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log"
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
	maxPages := 30
	seedURL := "https://en.wikipedia.org/wiki/Search_engine"

	var docs []Document
	var mu sync.Mutex
	visitedCount := 0

	c := colly.NewCollector(
		colly.MaxDepth(3),
		colly.AllowedDomains("en.wikipedia.org"),
		colly.Async(true),

		// Disallow standard Wikipedia utility namespaces, query parameters, and non-article paths
		colly.DisallowURLFilters(
			regexp.MustCompile(`(?i)/w/index\.php`),
			regexp.MustCompile(`(?i)/wiki/(Special|Talk|User|Wikipedia|File|MediaWiki|Template|Template_talk|Help|Portal|Category):`),
			regexp.MustCompile(`(?i)\?(action|printable|useskin)=`),
		),
	)

	// Rate limiting & politeness
	c.Limit(&colly.LimitRule{
		DomainGlob:  "*",
		Parallelism: 2,
		Delay:       500 * time.Millisecond,
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

		pageURL := e.Request.URL.String()
		title := strings.TrimSpace(e.ChildText("title"))

		// Basic text extraction (removing redundant whitespace)
		rawText := e.ChildText("p")
		content := strings.Join(strings.Fields(rawText), " ")

		// Collect outward links
		var links []string
		e.ForEach("a[href]", func(_ int, el *colly.HTMLElement) {
			link := el.Request.AbsoluteURL(el.Attr("href"))
			if link != "" {
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

		mu.Lock()
		docs = append(docs, doc)
		fmt.Printf("[%d/%d] Scraped: %s\n", currentCount, maxPages, pageURL)
		mu.Unlock()

		if currentCount < maxPages {
			for _, l := range links {
				e.Request.Visit(l)
			}
		}
	})

	c.OnRequest(func(r *colly.Request) {
		mu.Lock()
		defer mu.Unlock()
		if visitedCount >= maxPages {
			r.Abort()
		}
	})

	c.OnError(func(r *colly.Response, err error) {
		log.Printf("Error requesting %s: %v", r.Request.URL, err)
	})

	fmt.Printf("Starting filtered crawl at: %s\n", seedURL)
	c.Visit(seedURL)
	c.Wait()

	saveJSON("documents.json", docs)
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
