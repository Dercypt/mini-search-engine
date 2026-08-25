package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"os"
	"regexp"
	"strings"
)

type Document struct {
	ID      string   `json:"id"`
	URL     string   `json:"url"`
	Title   string   `json:"title"`
	Content string   `json:"content"`
	Links   []string `json:"links"`
}

type Posting struct {
	DocID     string `json:"doc_id"`
	Frequency int    `json:"freq"`
}

type InvertedIndex struct {
	Index        map[string][]Posting `json:"index"`
	DocLengths   map[string]int       `json:"doc_lengths"`
	DocCount     int                  `json:"doc_count"`
	AvgDocLength float64              `json:"avg_doc_length"`
}

var stopWords = map[string]bool{
	"the": true, "and": true, "to": true, "of": true, "a": true,
	"in": true, "that": true, "is": true, "was": true, "for": true,
	"on": true, "with": true, "as": true, "it": true, "by": true,
	"at": true, "an": true, "be": true, "this": true, "which": true,
}

func main() {
	inputPath := "../crawler/documents.jsonl"
	file, err := os.Open(inputPath)
	if err != nil {
		log.Fatalf("Couldn't find documents.jsonl: %v", err)
	}
	defer file.Close()

	reader := bufio.NewReader(file)
	index := make(map[string][]Posting)
	docLengths := make(map[string]int)
	docCount := 0
	totalLength := 0

	for {
		line, err := reader.ReadBytes('\n')
		if len(line) > 0 {
			var doc Document
			if err := json.Unmarshal(line, &doc); err == nil {
				docCount++
				tokens := tokenize(doc.Title + " " + doc.Content)
				docLen := len(tokens)
				docLengths[doc.ID] = docLen
				totalLength += docLen

				termFreqs := make(map[string]int)
				for _, token := range tokens {
					termFreqs[token]++
				}

				for term, freq := range termFreqs {
					index[term] = append(index[term], Posting{
						DocID:     doc.ID,
						Frequency: freq,
					})
				}
			}
		}

		if err != nil {
			if err == io.EOF {
				break
			}
			log.Printf("Error reading stream: %v", err)
			break
		}
	}

	avgDocLen := 0.0
	if docCount > 0 {
		avgDocLen = float64(totalLength) / float64(docCount)
	}

	invIndex := InvertedIndex{
		Index:        index,
		DocLengths:   docLengths,
		DocCount:     docCount,
		AvgDocLength: avgDocLen,
	}

	outputPath := "index.json"
	outData, err := json.Marshal(invIndex)
	if err != nil {
		log.Fatalf("Failed to serialize index: %v", err)
	}

	err = os.WriteFile(outputPath, outData, 0644)
	if err != nil {
		log.Fatalf("Failed to write index file: %v", err)
	}

	fmt.Printf("Successfully indexed %d documents. Output saved to %s\n", docCount, outputPath)
}

func tokenize(text string) []string {
	re := regexp.MustCompile(`[a-zA-Z0-9]+`)
	words := re.FindAllString(strings.ToLower(text), -1)

	var tokens []string
	for _, w := range words {
		if len(w) > 1 && !stopWords[w] {
			tokens = append(tokens, w)
		}
	}
	return tokens
}
