import { mount } from "svelte";
import "../lib/base.css";
import Meeting from "./Meeting.svelte";

mount(Meeting, { target: document.getElementById("app")! });
