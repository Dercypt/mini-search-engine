package main

import (
	"bufio"
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
	outputFile := "documents.jsonl"

	// Create/truncate file on start
	file, err := os.OpenFile(outputFile, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0644)
	if err != nil {
		log.Fatalf("Failed to open output file: %v", err)
	}
	defer file.Close()

	writer := bufio.NewWriter(file)
	defer writer.Flush()

	var mu sync.Mutex
	visitedCount := 0
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

	visitedURLs[seedURL] = true

	c.OnRequest(func(r *colly.Request) {
		mu.Lock()
		defer mu.Unlock()
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

			for _, re := range disallowed {
				if re.MatchString(cleanLink) {
					return
				}
			}

			links = append(links, cleanLink)

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

		// Stream document directly to disk
		mu.Lock()
		line, _ := json.Marshal(doc)
		writer.Write(line)
		writer.WriteString("\n")
		writer.Flush()
		fmt.Printf("[%d/%d] Scraped: %s\n", currentCount, maxPages, pageURL)

		if currentCount == maxPages {
			mu.Unlock()
			fmt.Printf("\nDone! Saved %d documents to %s\n", currentCount, outputFile)
			os.Exit(0)
		}
		mu.Unlock()

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
