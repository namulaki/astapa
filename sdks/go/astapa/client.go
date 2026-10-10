package astapa

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

// Config holds the configuration for the astapa client.
type Config struct {
	// APIURL is the base URL of the astapa API (e.g., "http://localhost:8080")
	APIURL string

	// ProxyHost is the hostname of the PostgreSQL proxy (e.g., "localhost")
	ProxyHost string

	// ProxyPort is the port of the PostgreSQL proxy (e.g., 5433)
	ProxyPort int

	// HTTPClient is an optional custom HTTP client. If nil, a default client is used.
	HTTPClient *http.Client
}

// Client is the astapa API client.
type Client struct {
	apiURL     string
	proxyHost  string
	proxyPort  int
	httpClient *http.Client
}

// NewClient creates a new astapa client with the given configuration.
func NewClient(cfg Config) *Client {
	httpClient := cfg.HTTPClient
	if httpClient == nil {
		httpClient = &http.Client{
			Timeout: 30 * time.Second,
		}
	}

	return &Client{
		apiURL:     cfg.APIURL,
		proxyHost:  cfg.ProxyHost,
		proxyPort:  cfg.ProxyPort,
		httpClient: httpClient,
	}
}

// CreateSession creates a new session for the given project.
func (c *Client) CreateSession(ctx context.Context, projectID string) (*SessionHandle, error) {
	reqBody := CreateSessionRequest{ProjectID: projectID}
	body, err := json.Marshal(reqBody)
	if err != nil {
		return nil, fmt.Errorf("marshal request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.apiURL+"/sessions", bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	var session Session
	if err := c.do(req, &session); err != nil {
		return nil, err
	}

	return &SessionHandle{
		Session: session,
		client:  c,
	}, nil
}

// GetSession retrieves an existing session by ID.
func (c *Client) GetSession(ctx context.Context, sessionID string) (*SessionHandle, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.apiURL+"/sessions/"+sessionID, nil)
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}

	var session Session
	if err := c.do(req, &session); err != nil {
		return nil, err
	}

	return &SessionHandle{
		Session: session,
		client:  c,
	}, nil
}

// do executes an HTTP request and decodes the JSON response.
func (c *Client) do(req *http.Request, v interface{}) error {
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("http request: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Errorf("read response: %w", err)
	}

	if resp.StatusCode >= 400 {
		apiErr := &APIError{StatusCode: resp.StatusCode}
		if err := json.Unmarshal(body, apiErr); err != nil {
			apiErr.Message = string(body)
		}
		return apiErr
	}

	if v != nil && len(body) > 0 {
		if err := json.Unmarshal(body, v); err != nil {
			return fmt.Errorf("decode response: %w", err)
		}
	}

	return nil
}
