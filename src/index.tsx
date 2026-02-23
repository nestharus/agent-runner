/* @refresh reload */

import { QueryClient, QueryClientProvider } from "@tanstack/solid-query";
import { render } from "solid-js/web";
import "./app.css";
import App from "./App";

const queryClient = new QueryClient();

const root = document.getElementById("app");
if (!root) throw new Error("Root element #app not found");

render(
	() => (
		<QueryClientProvider client={queryClient}>
			<App />
		</QueryClientProvider>
	),
	root,
);
