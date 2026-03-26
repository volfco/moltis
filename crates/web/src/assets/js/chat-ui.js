// ── Chat UI ─────────────────────────────────────────────────

import { formatTokens, parseErrorMessage, sendRpc, updateCountdown, renderMarkdown } from "./helpers.js";
import * as S from "./state.js";

function clearChatEmptyState() {
	if (!S.chatMsgBox) return;
	var welcome = S.chatMsgBox.querySelector("#welcomeCard");
	if (welcome) welcome.remove();
	var noProviders = S.chatMsgBox.querySelector("#noProvidersCard");
	if (noProviders) noProviders.remove();
	S.chatMsgBox.classList.remove("chat-messages-empty");
}

// Scroll chat to bottom and keep it pinned until layout settles.
// Uses a ResizeObserver to catch any late layout shifts (sidebar re-render,
// font loading, async style recalc) and re-scrolls until stable.
export function scrollChatToBottom() {
	if (!S.chatMsgBox) return;
	S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
	var box = S.chatMsgBox;
	var observer = new ResizeObserver(() => {
		box.scrollTop = box.scrollHeight;
	});
	observer.observe(box);
	setTimeout(() => {
		observer.disconnect();
	}, 500);
}

export function chatAddMsg(cls, content, isHtml, backendIndex, rawContent) {
	if (!S.chatMsgBox) return null;
	clearChatEmptyState();
	var el = document.createElement("div");
	el.className = `msg ${cls}`;
	if (cls === "system") {
		el.classList.add("system-notice");
	}

	var textContainer = document.createElement("div");
	textContainer.className = "msg-text-content";
	if (isHtml) {
		// Safe: content is produced by renderMarkdown which escapes via esc() first,
		// then only adds our own formatting tags (pre, code, strong).
		textContainer.innerHTML = content;
	} else {
		textContainer.textContent = content;
	}
	el.appendChild(textContainer);

	if (rawContent !== undefined) {
		el.dataset.rawContent = rawContent;
	} else if (!isHtml) {
		el.dataset.rawContent = content;
	}

	// Only add message ID and actions to real backend messages, not UI errors or transient system notes.
	var isRealMessage = backendIndex !== undefined || (cls !== "error" && cls !== "system");
	if (isRealMessage) {
		var messageIndex = backendIndex;
		if (messageIndex === undefined) {
			var maxId = -1;
			S.chatMsgBox.querySelectorAll("[data-message-id]").forEach((msgEl) => {
				var id = parseInt(msgEl.dataset.messageId, 10);
				if (!Number.isNaN(id) && id > maxId) maxId = id;
			});
			messageIndex = maxId + 1;
		}
		el.dataset.messageId = messageIndex.toString();

		// Add message footer and actions dropdown
		var footer = document.createElement("div");
		footer.className = "msg-model-footer";

		var actionsBtn = document.createElement("button");
		actionsBtn.className = "msg-actions-btn";
		actionsBtn.innerHTML = "⋮"; // Three dots
		actionsBtn.title = "Message actions";
		actionsBtn.onclick = function (e) {
			e.stopPropagation();
			toggleMessageActions(this);
		};
		footer.appendChild(actionsBtn);
		el.appendChild(footer);
	}

	S.chatMsgBox.appendChild(el);
	if (!S.chatBatchLoading) S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
	return el;
}

/**
 * Add a user message with image thumbnails below the text.
 * @param {string} cls - CSS class for the message (e.g. "user")
 * @param {string} htmlContent - Pre-rendered HTML text (from renderMarkdown)
 * @param {Array<{dataUrl: string, name: string}>} images - Images to display
 * @returns {HTMLElement|null}
 */
export function chatAddMsgWithImages(cls, htmlContent, images, backendIndex, rawContent) {
	if (!S.chatMsgBox) return null;
	clearChatEmptyState();
	var el = document.createElement("div");
	el.className = `msg ${cls}`;
	if (htmlContent) {
		var textDiv = document.createElement("div");
		textDiv.className = "msg-text-content";
		// Safe: htmlContent is produced by renderMarkdown which escapes user
		// input via esc() first, then only adds our own formatting tags.
		// This is the same pattern used in chatAddMsg above.
		textDiv.innerHTML = htmlContent; // eslint-disable-line no-unsanitized/property
		el.appendChild(textDiv);
	}
	if (rawContent !== undefined) {
		el.dataset.rawContent = rawContent;
	}
	if (images && images.length > 0) {
		var thumbRow = document.createElement("div");
		thumbRow.className = "msg-image-row";
		for (var img of images) {
			var thumb = document.createElement("img");
			thumb.className = "msg-image-thumb";
			thumb.src = img.dataUrl;
			thumb.alt = img.name;
			thumbRow.appendChild(thumb);
		}
		el.appendChild(thumbRow);
	}

	// Only add message ID and actions to real backend messages, not UI errors or transient system notes.
	var isRealMessage = backendIndex !== undefined || (cls !== "error" && cls !== "system");
	if (isRealMessage) {
		var messageIndex = backendIndex;
		if (messageIndex === undefined) {
			var maxId = -1;
			S.chatMsgBox.querySelectorAll("[data-message-id]").forEach((msgEl) => {
				var id = parseInt(msgEl.dataset.messageId, 10);
				if (!Number.isNaN(id) && id > maxId) maxId = id;
			});
			messageIndex = maxId + 1;
		}
		el.dataset.messageId = messageIndex.toString();

		// Add message footer and actions dropdown
		var footer = document.createElement("div");
		footer.className = "msg-model-footer";

		var actionsBtn = document.createElement("button");
		actionsBtn.className = "msg-actions-btn";
		actionsBtn.innerHTML = "⋮"; // Three dots
		actionsBtn.title = "Message actions";
		actionsBtn.onclick = function (e) {
			e.stopPropagation();
			toggleMessageActions(this);
		};
		footer.appendChild(actionsBtn);
		el.appendChild(footer);
	}

	S.chatMsgBox.appendChild(el);
	if (!S.chatBatchLoading) S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
	return el;
}

export function stripChannelPrefix(text) {
	return text.replace(/^\[Telegram(?:\s+from\s+[^\]]+)?\]\s*/, "");
}

export function appendChannelFooter(el, channel) {
	var ft = document.createElement("div");
	ft.className = "msg-channel-footer";
	var label = channel.channel_type || "channel";
	var who = channel.username ? `@${channel.username}` : channel.sender_name;
	if (who) label += ` \u00b7 ${who}`;
	if (channel.message_kind === "voice") {
		var icon = document.createElement("span");
		icon.className = "voice-icon";
		icon.setAttribute("aria-hidden", "true");
		ft.appendChild(icon);
	}

	var text = document.createElement("span");
	text.textContent = `via ${label}`;
	ft.appendChild(text);
	el.appendChild(ft);
}

export function removeThinking() {
	var el = document.getElementById("thinkingIndicator");
	if (el) el.remove();
}

export function appendReasoningDisclosure(messageEl, reasoningText) {
	if (!messageEl) return null;
	var normalized = String(reasoningText || "").trim();
	if (!normalized) return null;
	var existing = messageEl.querySelector(".msg-reasoning");
	if (existing) existing.remove();
	var details = document.createElement("details");
	details.className = "msg-reasoning";
	var summary = document.createElement("summary");
	summary.className = "msg-reasoning-summary";
	summary.textContent = "Reasoning";
	details.appendChild(summary);
	var body = document.createElement("div");
	body.className = "msg-reasoning-body";
	body.textContent = normalized;
	details.appendChild(body);
	messageEl.appendChild(details);
	return details;
}

export function chatAddErrorCard(err) {
	if (!S.chatMsgBox) return;
	clearChatEmptyState();
	var el = document.createElement("div");
	el.className = "msg error-card";

	var icon = document.createElement("div");
	icon.className = "error-icon";
	icon.textContent = err.icon || "\u26A0\uFE0F";
	el.appendChild(icon);

	var body = document.createElement("div");
	body.className = "error-body";

	var title = document.createElement("div");
	title.className = "error-title";
	title.textContent = err.title;
	body.appendChild(title);

	if (err.detail) {
		var detail = document.createElement("div");
		detail.className = "error-detail";
		detail.textContent = err.detail;
		body.appendChild(detail);
	}

	if (err.provider) {
		var prov = document.createElement("div");
		prov.className = "error-detail";
		prov.textContent = `Provider: ${err.provider}`;
		prov.style.marginTop = "4px";
		prov.style.opacity = "0.6";
		body.appendChild(prov);
	}

	if (err.resetsAt) {
		var countdown = document.createElement("div");
		countdown.className = "error-countdown";
		el.appendChild(body);
		el.appendChild(countdown);
		updateCountdown(countdown, err.resetsAt);
		var timer = setInterval(() => {
			if (updateCountdown(countdown, err.resetsAt)) clearInterval(timer);
		}, 1000);
	} else {
		el.appendChild(body);
	}

	S.chatMsgBox.appendChild(el);
	S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
}

export function chatAddErrorMsg(message) {
	chatAddErrorCard(parseErrorMessage(message));
}

export function renderApprovalCard(requestId, command) {
	if (!S.chatMsgBox) return;
	clearChatEmptyState();
	var tpl = document.getElementById("tpl-approval-card");
	var frag = tpl.content.cloneNode(true);
	var card = frag.firstElementChild;
	card.id = `approval-${requestId}`;

	card.querySelector(".approval-cmd").textContent = command;

	var allowBtn = card.querySelector(".approval-allow");
	var denyBtn = card.querySelector(".approval-deny");
	allowBtn.onclick = () => {
		resolveApproval(requestId, "approved", command, card);
	};
	denyBtn.onclick = () => {
		resolveApproval(requestId, "denied", null, card);
	};

	var countdown = card.querySelector(".approval-countdown");
	var remaining = 120;
	var timer = setInterval(() => {
		remaining--;
		countdown.textContent = `${remaining}s`;
		if (remaining <= 0) {
			clearInterval(timer);
			card.classList.add("approval-expired");
			allowBtn.disabled = true;
			denyBtn.disabled = true;
			countdown.textContent = "expired";
		}
	}, 1000);
	countdown.textContent = `${remaining}s`;

	S.chatMsgBox.appendChild(card);
	S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
}

export function resolveApproval(requestId, decision, command, card) {
	var params = { requestId: requestId, decision: decision };
	if (command) params.command = command;
	sendRpc("exec.approval.resolve", params).then(() => {
		card.classList.add("approval-resolved");
		card.querySelectorAll(".approval-btn").forEach((b) => {
			b.disabled = true;
		});
		var status = document.createElement("div");
		status.className = "approval-status";
		status.textContent = decision === "approved" ? "Allowed" : "Denied";
		card.appendChild(status);
	});
}

export function highlightAndScroll(msgEls, messageIndex, query) {
	var target = null;
	if (messageIndex >= 0 && messageIndex < msgEls.length && msgEls[messageIndex]) {
		target = msgEls[messageIndex];
	}
	var lowerQ = query.toLowerCase();
	if (!target || (target.textContent || "").toLowerCase().indexOf(lowerQ) === -1) {
		for (var candidate of msgEls) {
			if (candidate && (candidate.textContent || "").toLowerCase().indexOf(lowerQ) !== -1) {
				target = candidate;
				break;
			}
		}
	}
	if (!target) return;
	msgEls.forEach((el) => {
		if (el) highlightTermInElement(el, query);
	});
	target.scrollIntoView({ behavior: "smooth", block: "center" });
	target.classList.add("search-highlight-msg");
	setTimeout(() => {
		if (!S.chatMsgBox) return;
		S.chatMsgBox.querySelectorAll("mark.search-term-highlight").forEach((m) => {
			var parent = m.parentNode;
			parent.replaceChild(document.createTextNode(m.textContent), m);
			parent.normalize();
		});
		S.chatMsgBox.querySelectorAll(".search-highlight-msg").forEach((el) => {
			el.classList.remove("search-highlight-msg");
		});
	}, 5000);
}

export function highlightTermInElement(el, query) {
	var walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, null, false);
	var nodes = [];
	while (walker.nextNode()) nodes.push(walker.currentNode);
	var lowerQ = query.toLowerCase();
	nodes.forEach((textNode) => {
		var text = textNode.nodeValue;
		var lowerText = text.toLowerCase();
		var idx = lowerText.indexOf(lowerQ);
		if (idx === -1) return;
		var frag = document.createDocumentFragment();
		var pos = 0;
		while (idx !== -1) {
			if (idx > pos) frag.appendChild(document.createTextNode(text.substring(pos, idx)));
			var mark = document.createElement("mark");
			mark.className = "search-term-highlight";
			mark.textContent = text.substring(idx, idx + query.length);
			frag.appendChild(mark);
			pos = idx + query.length;
			idx = lowerText.indexOf(lowerQ, pos);
		}
		if (pos < text.length) frag.appendChild(document.createTextNode(text.substring(pos)));
		textNode.parentNode.replaceChild(frag, textNode);
	});
}

export function chatAutoResize() {
	if (!S.chatInput) return;
	S.chatInput.style.height = "auto";
	S.chatInput.style.height = `${Math.min(S.chatInput.scrollHeight, 120)}px`;
}

export function updateCommandInputUI() {
	if (!S.chatInput) return;
	var row = S.chatInput.closest(".chat-input-row");
	if (row) {
		row.classList.toggle("command-mode", S.commandModeEnabled);
	}
	var prompt = S.$("chatCommandPrompt");
	if (prompt) {
		prompt.textContent = S.sessionExecPromptSymbol || "$";
		prompt.classList.toggle("chat-command-prompt-hidden", !S.commandModeEnabled);
		prompt.setAttribute("aria-hidden", S.commandModeEnabled ? "false" : "true");
	}
	if (S.commandModeEnabled) {
		S.chatInput.placeholder = "Run shell command\u2026";
		S.chatInput.setAttribute("aria-label", "Command input");
	} else {
		S.chatInput.placeholder = "Type a message...";
		S.chatInput.setAttribute("aria-label", "Chat input");
	}
	updateTokenBar();
}

export function updateTokenBar() {
	var bar = S.$("tokenBar");
	if (!bar) return;
	var total = S.sessionTokens.input + S.sessionTokens.output;
	var text =
		formatTokens(S.sessionTokens.input) +
		" in / " +
		formatTokens(S.sessionTokens.output) +
		" out \u00b7 " +
		formatTokens(total) +
		" tokens";
	if (S.sessionContextWindow > 0) {
		var currentInput = S.sessionCurrentInputTokens || 0;
		var pct = Math.max(0, 100 - Math.round((currentInput / S.sessionContextWindow) * 100));
		text += ` \u00b7 Context left before auto-compact: ${pct}%`;
	}
	if (!S.sessionToolsEnabled) {
		text += " \u00b7 Tools: disabled";
	}
	var execModeLabel = S.sessionExecMode === "sandbox" ? "sandboxed" : "host";
	var promptSymbol = S.sessionExecPromptSymbol || "$";
	text += ` \u00b7 Execute: ${execModeLabel} (${promptSymbol})`;
	if (S.commandModeEnabled) {
		text += " \u00b7 /sh mode";
	}
	bar.textContent = text;
}

// Message actions dropdown
function toggleMessageActions(button) {
	var wasOpen = button.classList.contains("menu-open");

	// Remove any existing dropdowns
	document.querySelectorAll(".msg-actions-dropdown").forEach((dropdown) => {
		if (typeof dropdown.closeDropdown === "function") {
			dropdown.closeDropdown();
		} else {
			dropdown.remove();
		}
	});
	document.querySelectorAll(".msg-actions-btn").forEach((btn) => {
		btn.classList.remove("menu-open");
	});

	if (wasOpen) {
		return;
	}

	button.classList.add("menu-open");

	// Create dropdown
	var dropdown = document.createElement("div");
	dropdown.className = "msg-actions-dropdown";

	function closeDropdown() {
		dropdown.remove();
		button.classList.remove("menu-open");
		document.removeEventListener("click", closeOnClickOutside);
		document.removeEventListener("keydown", closeOnEscape);
	}
	dropdown.closeDropdown = closeDropdown;

	// Edit option
	var editOption = document.createElement("div");
	editOption.className = "msg-actions-option";
	editOption.textContent = "Edit";
	editOption.onclick = (e) => {
		e.stopPropagation();
		closeDropdown();

		var messageEl = button.closest(".msg");
		if (!messageEl) return;

		var messageId = messageEl.dataset.messageId;
		if (!messageId) {
			alert("Cannot edit this message.");
			return;
		}

		var textContainer = messageEl.querySelector(".msg-text-content");
		if (!textContainer) {
			alert("Could not find message text to edit.");
			return;
		}

		var rawContent = messageEl.dataset.rawContent || "";

		// Hide original text content
		textContainer.style.display = "none";

		// Create edit container
		var editContainer = document.createElement("div");
		editContainer.className = "msg-edit-container";
		editContainer.style.width = "100%";
		editContainer.style.marginTop = "8px";

		var textarea = document.createElement("textarea");
		textarea.className = "w-full bg-[var(--surface2)] border border-[var(--border)] text-[var(--text)] px-3 py-2 rounded-lg text-sm resize-y min-h-[100px] leading-relaxed focus:outline-none focus:border-[var(--border-strong)] focus:ring-1 focus:ring-[var(--accent-subtle)] transition-colors font-[var(--font-body)]";
		textarea.style.marginBottom = "8px";
		textarea.value = rawContent;

		var btnRow = document.createElement("div");
		btnRow.style.display = "flex";
		btnRow.style.gap = "8px";
		btnRow.style.justifyContent = "flex-end";

		var saveBtn = document.createElement("button");
		saveBtn.className = "provider-btn provider-btn-sm";
		saveBtn.textContent = "Save";

		var cancelBtn = document.createElement("button");
		cancelBtn.className = "provider-btn provider-btn-secondary provider-btn-sm";
		cancelBtn.textContent = "Cancel";

		btnRow.appendChild(cancelBtn);
		btnRow.appendChild(saveBtn);
		editContainer.appendChild(textarea);
		editContainer.appendChild(btnRow);

		// Insert before the text container so it takes its place visually
		textContainer.parentNode.insertBefore(editContainer, textContainer.nextSibling);

		// Focus and move cursor to end
		textarea.focus();
		textarea.selectionStart = textarea.value.length;

		function closeEdit() {
			editContainer.remove();
			textContainer.style.display = "";
		}

		cancelBtn.onclick = () => {
			closeEdit();
		};

		saveBtn.onclick = () => {
			var newContent = textarea.value;
			if (newContent === rawContent) {
				closeEdit();
				return;
			}

			saveBtn.disabled = true;
			saveBtn.textContent = "Saving...";

			sendRpc("chat.edit_message", { messageId: messageId, content: newContent })
				.then(() => {
					// Update local state
					messageEl.dataset.rawContent = newContent;
					
					// Re-render HTML content
					textContainer.innerHTML = renderMarkdown(newContent); // eslint-disable-line no-unsanitized/property
					closeEdit();
				})
				.catch((err) => {
					alert("Failed to edit message: " + (err.message || "Unknown error"));
					saveBtn.disabled = false;
					saveBtn.textContent = "Save";
				});
		};
	};

	// Delete option
	var deleteOption = document.createElement("div");
	deleteOption.className = "msg-actions-option";
	deleteOption.textContent = "Delete";
	deleteOption.onclick = (e) => {
		e.stopPropagation();
		if (confirm("Are you sure you want to delete this message? This action cannot be undone.")) {
			// Find the parent message element
			var messageEl = button.closest(".msg");
			if (messageEl) {
				// Get message ID for backend deletion
				var messageId = messageEl.dataset.messageId;
				if (messageId) {
					// Remove from frontend
					messageEl.remove();

					// Re-index all remaining messages that had an ID greater than the deleted one
					var deletedId = parseInt(messageId, 10);
					document.querySelectorAll(".msg[data-message-id]").forEach((el) => {
						var currentId = parseInt(el.dataset.messageId, 10);
						if (currentId > deletedId) {
							el.dataset.messageId = (currentId - 1).toString();
						}
					});

					// Send RPC to backend to delete message
					sendRpc("chat.delete_message", { messageId: messageId });
				} else {
					// Fallback: just remove from frontend if no ID
					messageEl.remove();
				}
			}
		}
		closeDropdown();
	};

	dropdown.appendChild(editOption);
	dropdown.appendChild(deleteOption);

	// Position dropdown below the button
	var buttonRect = button.getBoundingClientRect();
	dropdown.style.position = "absolute";
	dropdown.style.left = buttonRect.left + window.scrollX + "px";
	dropdown.style.top = buttonRect.bottom + window.scrollY + "px";
	dropdown.style.zIndex = "1000";

	document.body.appendChild(dropdown);

	// Close dropdown when clicking outside
	function closeOnClickOutside(e) {
		if (!dropdown.contains(e.target) && e.target !== button) {
			closeDropdown();
		}
	}

	// Close on escape key
	function closeOnEscape(e) {
		if (e.key === "Escape") {
			closeDropdown();
		}
	}

	document.addEventListener("click", closeOnClickOutside);
	document.addEventListener("keydown", closeOnEscape);
}
