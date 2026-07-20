const form = document.querySelector("#submission-form");
const printerNotice = document.querySelector("#printer-notice");
const printerMessage = document.querySelector("#printer-message");
const messageInput = document.querySelector("#message");
const characterCount = document.querySelector("#character-count");
const authorInput = document.querySelector("#author");
const uploadInput = document.querySelector("#upload-input");
const cameraInput = document.querySelector("#camera-input");
const uploadButton = document.querySelector("#upload-button");
const cameraButton = document.querySelector("#camera-button");
const imagePreview = document.querySelector("#image-preview");
const previewImage = document.querySelector("#preview-image");
const imageName = document.querySelector("#image-name");
const removeImageButton = document.querySelector("#remove-image");
const submitButton = document.querySelector("#submit-button");
const formStatus = document.querySelector("#form-status");

const MAX_IMAGE_BYTES = 8 * 1024 * 1024;
const MAX_IMAGE_DIMENSION = 1024;
let selectedImage = null;
let previewUrl = null;

function removeUnsupportedCharacters(input, allowNewlines = false) {
    const unsupported = allowNewlines ? /[^\x20-\x7E\n]/g : /[^\x20-\x7E]/g;
    const cleaned = input.value.replace(unsupported, "");

    if (cleaned !== input.value) {
        const cursor = input.selectionStart;
        const removedBeforeCursor = input.value
            .slice(0, cursor)
            .replace(/[\x20-\x7E\n]/g, "").length;

        input.value = cleaned;
        input.setSelectionRange(cursor - removedBeforeCursor, cursor - removedBeforeCursor);
        setFormStatus("Emoji and unsupported symbols have been removed.", "error");
    }
}

function updateCharacterCount() {
    characterCount.textContent = `${messageInput.value.length} / 500`;
}

function setFormStatus(message, kind = "") {
    formStatus.textContent = message;
    formStatus.className = `form-status ${kind}`.trim();
}

function clearImage() {
    if (previewUrl) URL.revokeObjectURL(previewUrl);

    selectedImage = null;
    previewUrl = null;
    uploadInput.value = "";
    cameraInput.value = "";
    previewImage.removeAttribute("src");
    imageName.textContent = "";
    imagePreview.hidden = true;
}

function chooseImage(file) {
    if (!file) return;

    if (!file.type.startsWith("image/")) {
        setFormStatus("Please choose an image file.", "error");
        return;
    }

    if (file.size > MAX_IMAGE_BYTES) {
        setFormStatus("Please choose an image smaller than 8 MB.", "error");
        return;
    }

    clearImage();
    selectedImage = file;
    previewUrl = URL.createObjectURL(file);
    previewImage.src = previewUrl;
    imageName.textContent = file.name || "Camera image";
    imagePreview.hidden = false;
    setFormStatus("");
}

async function imageAsJpeg(file) {
    const bitmap = await createImageBitmap(file, { imageOrientation: "from-image" });
    const scale = Math.min(1, MAX_IMAGE_DIMENSION / Math.max(bitmap.width, bitmap.height));
    const width = Math.max(1, Math.round(bitmap.width * scale));
    const height = Math.max(1, Math.round(bitmap.height * scale));
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;

    const context = canvas.getContext("2d");
    context.fillStyle = "#ffffff";
    context.fillRect(0, 0, width, height);
    context.drawImage(bitmap, 0, 0, width, height);
    bitmap.close();

    return new Promise((resolve, reject) => {
        canvas.toBlob(
            (blob) => blob ? resolve(blob) : reject(new Error("The image could not be prepared.")),
            "image/jpeg",
            0.88,
        );
    });
}

function showPrinterUnavailable(message) {
    printerMessage.textContent = message || "Please check back in a little while.";
    printerNotice.hidden = false;
    form.hidden = true;
}

async function checkPrinter() {
    try {
        const response = await fetch("/api/printer/status", {
            headers: { accept: "application/json" },
        });
        const status = await response.json();

        if (!response.ok || !status.available || !status.paper_present) {
            showPrinterUnavailable(status.message);
        }
    } catch {
        showPrinterUnavailable("We cannot reach the printer right now. Please try again later.");
    }
}

messageInput.addEventListener("input", () => {
    removeUnsupportedCharacters(messageInput, true);
    updateCharacterCount();
});
authorInput.addEventListener("input", () => removeUnsupportedCharacters(authorInput));
uploadButton.addEventListener("click", () => uploadInput.click());
cameraButton.addEventListener("click", () => cameraInput.click());
uploadInput.addEventListener("change", () => chooseImage(uploadInput.files[0]));
cameraInput.addEventListener("change", () => chooseImage(cameraInput.files[0]));
removeImageButton.addEventListener("click", clearImage);

form.addEventListener("submit", async (event) => {
    event.preventDefault();
    setFormStatus("");

    if (!form.reportValidity()) return;

    submitButton.disabled = true;
    submitButton.textContent = "Sending…";

    try {
        const submission = {
            message: messageInput.value.trim(),
            author: authorInput.value.trim(),
        };
        const multipart = new FormData();
        multipart.append(
            "submission",
            new Blob([JSON.stringify(submission)], { type: "application/json" }),
        );

        if (selectedImage) {
            const jpeg = await imageAsJpeg(selectedImage);
            multipart.append("image", jpeg, "submission.jpg");
        }

        const response = await fetch("/api/print", {
            method: "POST",
            body: multipart,
        });
        const result = await response.json().catch(() => ({}));

        if (!response.ok) {
            throw new Error(result.error || "Your submission could not be printed.");
        }

        form.reset();
        clearImage();
        updateCharacterCount();
        setFormStatus(result.message || "Your quote was sent to the wall.", "success");
        messageInput.focus();
    } catch (error) {
        setFormStatus(error instanceof Error ? error.message : "Something went wrong.", "error");
    } finally {
        submitButton.disabled = false;
        submitButton.textContent = "Memorialize the quote";
    }
});

updateCharacterCount();
checkPrinter();
