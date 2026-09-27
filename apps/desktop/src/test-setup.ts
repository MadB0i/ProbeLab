// Makes the DOM matchers available without importing them in every file. A test that has to remember
// `import "@testing-library/jest-dom"` in each module is a test suite that will eventually forget one.
import "@testing-library/jest-dom/vitest";
