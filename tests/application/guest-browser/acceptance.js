// Served only by the disposable guest browser fixture.
const form = document.querySelector("#acceptance");
const terms = document.querySelector("#terms");
const submit = document.querySelector("#submit-order");
const result = document.querySelector("#receipt");
let intent;
let csrf;

window.configureSyntheticAcceptance = (order, token) => {
  intent = structuredClone(order);
  csrf = token;
  submit.disabled = false;
};

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!intent || !csrf || !terms.checked) return;
  submit.disabled = true;
  result.textContent = "Submitting synthetic order…";
  try {
    const response = await fetch("/api/test/guest/orders", {
      method: "POST",
      credentials: "same-origin",
      headers: { "content-type": "application/json", "x-csrf-token": csrf },
      body: JSON.stringify({ ...intent, confirmed: terms.checked }),
    });
    if (!response.ok) throw new Error("Receipt unavailable");
    const receipt = await response.json();
    result.textContent = `Synthetic order receipt: ${receipt.id}`;
    result.dataset.replayed = String(receipt.replayed);
  } catch {
    result.textContent = "Receipt unavailable. Retry the same synthetic order.";
    submit.disabled = false;
  }
});
